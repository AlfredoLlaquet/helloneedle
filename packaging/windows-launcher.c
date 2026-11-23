/* SPDX-License-Identifier: GPL-3.0-or-later */
/* Use only Windows system DLLs so Explorer can report a missing Qt runtime. */
#ifndef UNICODE
#define UNICODE
#endif
#ifndef _UNICODE
#define _UNICODE
#endif
#include <windows.h>
#include <strsafe.h>
#include <wchar.h>

enum { capacity = 32768 };
static wchar_t app[capacity], command[capacity], runtime[capacity], path[capacity], original_path[capacity];

static BOOL contains_qt(const wchar_t *bin)
{
    wchar_t dll[capacity];
    if (FAILED(StringCchPrintfW(dll, capacity, L"%ls\\Qt6Widgets.dll", bin)))
        return FALSE;
    return GetFileAttributesW(dll) != INVALID_FILE_ATTRIBUTES;
}

static BOOL runtime_from(const wchar_t *variable, const wchar_t *suffix)
{
    DWORD size = GetEnvironmentVariableW(variable, runtime, capacity);
    return size && size < capacity && SUCCEEDED(StringCchCatW(runtime, capacity, suffix)) && contains_qt(runtime);
}

static BOOL find_runtime(void)
{
    if (runtime_from(L"HELLONEEDLE_UCRT64", L"\\bin") || runtime_from(L"SystemDrive", L"\\msys64\\ucrt64\\bin"))
        return TRUE;
    /* An existing UCRT64 shell can use a custom MSYS2 installation. */
    wchar_t search[capacity];
    StringCchCopyW(search, capacity, original_path);
    wchar_t *entry = search;
    while (entry) {
        wchar_t *next = wcschr(entry, L';');
        if (next)
            *next++ = L'\0';
        const wchar_t *suffix = L"\\ucrt64\\bin";
        size_t size = wcslen(entry), tail = wcslen(suffix);
        if (size > tail && !_wcsicmp(entry + size - tail, suffix) && contains_qt(entry))
            return SUCCEEDED(StringCchCopyW(runtime, capacity, entry));
        entry = next;
    }
    return FALSE;
}

static int failure(const wchar_t *english, const wchar_t *spanish, BOOL unattended)
{
    if (!unattended)
        MessageBoxW(NULL, PRIMARYLANGID(GetUserDefaultUILanguage()) == LANG_SPANISH ? spanish : english,
                    L"Hello Needle", MB_OK | MB_ICONERROR);
    return 1;
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR arguments, int show)
{
    (void)instance; (void)previous; (void)show;
    BOOL unattended = wcscmp(arguments, L"--smoke-test") == 0;
    DWORD length = GetModuleFileNameW(NULL, app, capacity);
    wchar_t *name = wcsrchr(app, L'\\');
    if (!length || length >= capacity || !name)
        return failure(L"The application folder could not be located.",
                       L"No se ha encontrado la carpeta de la aplicación.", unattended);
    *name = L'\0';
    if (FAILED(StringCchCatW(app, capacity, L"\\bin\\helloneedle.exe")) ||
        GetFileAttributesW(app) == INVALID_FILE_ATTRIBUTES)
        return failure(L"Extract the entire Hello Needle ZIP, including its bin and share folders.",
                       L"Extraiga el archivo ZIP de Hello Needle completo, incluidas las carpetas bin y share.", unattended);

    if (GetEnvironmentVariableW(L"PATH", original_path, capacity) >= capacity || !find_runtime())
        return failure(L"Install the MSYS2 UCRT64 runtime packages listed in WINDOWS.txt. "
                       L"For a custom installation, set HELLONEEDLE_UCRT64 to the full ucrt64 folder.",
                       L"Instale los paquetes de MSYS2 UCRT64 indicados en WINDOWS.txt. Si ha instalado MSYS2 "
                       L"en otra ubicación, asigne a HELLONEEDLE_UCRT64 la ruta completa a la carpeta ucrt64.", unattended);
    if (FAILED(StringCchPrintfW(path, capacity, L"%ls;%ls", runtime, original_path)) ||
        !SetEnvironmentVariableW(L"PATH", path) ||
        FAILED(StringCchPrintfW(command, capacity, L"\"%ls\" %ls", app, arguments)))
        return failure(L"The runtime path or command line is too long.",
                       L"La ruta del entorno de ejecución o la línea de comandos es demasiado larga.", unattended);

    STARTUPINFOW startup = {0};
    PROCESS_INFORMATION process = {0};
    startup.cb = sizeof(startup);
    if (!CreateProcessW(app, command, NULL, NULL, FALSE, 0, NULL, NULL, &startup, &process))
        return failure(L"Windows could not start Hello Needle. See WINDOWS.txt for the runtime packages.",
                       L"Windows no ha podido iniciar Hello Needle. Consulte la lista de paquetes en WINDOWS.txt.", unattended);
    CloseHandle(process.hThread);
    DWORD result = 1;
    WaitForSingleObject(process.hProcess, INFINITE);
    GetExitCodeProcess(process.hProcess, &result);
    CloseHandle(process.hProcess);
    return (int)result;
}
