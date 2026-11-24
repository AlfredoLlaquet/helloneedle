# SPDX-License-Identifier: GPL-3.0-or-later
# Run from native PowerShell 7 x64 or ARM64 in a disposable Windows user profile.
# The installer, published executables and document helpers must work without MSYS2.
#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$ExpectedVersion,
    [Parameter(Mandatory)][string]$Python,
    [string]$EvidenceDirectory = 'native-out/installed-evidence',
    [string]$GuiTester
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Run in native 64-bit Windows PowerShell 7.'
}
$setup = (Resolve-Path -LiteralPath $Installer).ProviderPath
$pythonExe = (Resolve-Path -LiteralPath $Python).ProviderPath
if ($pythonExe -match '\\(msys\d*|ucrt64|mingw64)\\') {
    throw 'Provide ordinary Windows Python on the clean test machine.'
}
$tester = if ($GuiTester) { (Resolve-Path -LiteralPath $GuiTester).ProviderPath }
$evidence = [IO.Path]::GetFullPath($EvidenceDirectory)
[void][IO.Directory]::CreateDirectory($evidence)
$repo = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..\..')).ProviderPath
$runtimeTest = Join-Path $PSScriptRoot 'runtime-check.py'
$work = Join-Path ([IO.Path]::GetTempPath()) ('Hello Needle installer ' + [guid]::NewGuid())
$documents = Join-Path $work 'Original files'
$directory = Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'Programs\Hello Needle'
$startLink = Join-Path ([Environment]::GetFolderPath('Programs')) 'Hello Needle\Hello Needle.lnk'
$desktopLink = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'Hello Needle.lnk'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\org.thecrumbs.HelloNeedle_is1'
$machine = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
$account = [Microsoft.Win32.RegistryKey]::OpenBaseKey('CurrentUser', 'Registry64')
$inventory = Join-Path $evidence 'original-document-hashes.json'
$systemPath = "$env:SystemRoot\System32;$env:SystemRoot"
$settings = @{}
$stage = 'Preparation'
$finished = [Collections.Generic.List[string]]::new()
$ownedInstallation = $false
$installed = [ordered]@{}
$registeredBefore = $null

function Get-Registration($Hive) {
    $key = $Hive.OpenSubKey($uninstallKey)
    if (-not $key) { return $null }
    try {
        $v = @{}
        foreach ($name in $key.GetValueNames()) { $v[$name] = $key.GetValue($name) }
        return $v
    } finally { $key.Dispose() }
}

function Same-Path([string]$Left, [string]$Right) {
    if (-not $Left -or -not $Right) { return $false }
    return [string]::Equals([IO.Path]::GetFullPath($Left).TrimEnd('\'),
        [IO.Path]::GetFullPath($Right).TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
}

function Invoke-Checked([string]$Path, [string[]]$Arguments, [string]$Name, [int]$Timeout = 120) {
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Path
    $start.WorkingDirectory = $work
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = $start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    foreach ($arg in $Arguments) { [void]$start.ArgumentList.Add($arg) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($Timeout * 1000)
        if ($timedOut) {
            $process.Kill($true)
            $process.WaitForExit()
        }
        $log = $stdout.GetAwaiter().GetResult() + $stderr.GetAwaiter().GetResult()
        [IO.File]::WriteAllText((Join-Path $evidence "$Name.log"), $log)
        if ($timedOut) { throw "$Name timed out after $Timeout seconds. $log" }
        if ($process.ExitCode -ne 0) { throw "$Name exited $($process.ExitCode). $log" }
        return $log
    } finally { $process.Dispose() }
}

function Check-Shortcut([string]$Link, [string]$Target) {
    if (-not [IO.File]::Exists($Link)) { throw "Missing Shell shortcut: $Link" }
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($Link)
        try {
            if (-not (Same-Path $shortcut.TargetPath $Target) -or $shortcut.Arguments) {
                throw "Shortcut does not directly launch Hello Needle: $Link"
            }
            # IconLocation can be omitted to inherit the executable's icon.
            $icon = $shortcut.IconLocation -replace ',\s*-?\d+$',''
            if ($icon -and -not (Same-Path $icon $Target)) { throw "Wrong Shell icon for $Link" }
        } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
    } finally { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell) }
}

function Check-ShellLaunch([string]$Link, [string]$DesktopExecutable) {
    if (-not ('HelloNeedleInstaller.ExitStatus' -as [type])) {
        Add-Type -TypeDefinition @'
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
namespace HelloNeedleInstaller {
    public static class ExitStatus {
        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool GetExitCodeProcess(SafeProcessHandle process, out uint code);
        public static uint Read(SafeProcessHandle process) {
            uint code;
            if (!GetExitCodeProcess(process, out code)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return code;
        }
    }
}
'@
    }
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Link
    $start.UseShellExecute = $true
    $shellProcess = [Diagnostics.Process]::Start($start)
    $gui = $null
    $launcherProcess = $null
    $report = [ordered]@{shortcut=$Link}
    try {
        $deadline = [DateTime]::UtcNow.AddSeconds(35)
        do {
            if (-not $gui) {
                foreach ($candidate in [Diagnostics.Process]::GetProcessesByName('helloneedle')) {
                    # Windows can enumerate a new GUI process before its executable module is ready.
                    $module = $candidate.MainModule
                    if (-not $gui -and $module -and (Same-Path $module.FileName $DesktopExecutable)) {
                        $gui = $candidate
                        # An enumerated Process does not retain a kernel handle until
                        # requested. Once the PID disappears its ExitCode can be null.
                        $guiHandle = $gui.SafeHandle
                    } else { $candidate.Dispose() }
                }
            }
            if ($gui) {
                $gui.Refresh()
                if ($gui.HasExited) { throw 'Start menu launch exited before displaying the desktop application.' }
                if ($gui.MainWindowHandle -ne [IntPtr]::Zero -and $gui.MainWindowTitle -eq 'Hello Needle') { break }
            }
            Start-Sleep -Milliseconds 200
        } while ([DateTime]::UtcNow -lt $deadline)
        if (-not $gui -or $gui.MainWindowHandle -eq [IntPtr]::Zero -or $gui.MainWindowTitle -ne 'Hello Needle') {
            throw 'The installed Start menu shortcut did not open a visible Hello Needle window.'
        }
        $loaded = @{}
        foreach ($module in $gui.Modules) { $loaded[$module.ModuleName] = $module.FileName }
        foreach ($relative in 'bin\Qt6Core.dll','bin\Qt6Gui.dll','bin\Qt6Widgets.dll','plugins\platforms\qwindows.dll') {
            $name = [IO.Path]::GetFileName($relative)
            if (-not (Same-Path $loaded[$name] (Join-Path $directory $relative))) {
                throw "The Start menu application loaded a different $name runtime: $($loaded[$name])"
            }
        }
        $modules = @($loaded.Values | Where-Object { $_ -match '\\(Qt6[^\\]+|qwindows|qmodernwindowsstyle)\.dll$' } | Sort-Object)
        $report.windowTitle = $gui.MainWindowTitle
        $report.executable = $gui.MainModule.FileName
        $report.modules = $modules
        $report.guiPid = $gui.Id
        $launcherPath = Join-Path $directory 'Hello Needle.exe'
        # ShellExecute may return its broker instead of the executable. Only wait
        # for the verified installed launcher, which explicitly returns the GUI status.
        foreach ($candidate in [Diagnostics.Process]::GetProcessesByName('Hello Needle')) {
            $module = $candidate.MainModule
            if (-not $launcherProcess -and $module -and (Same-Path $module.FileName $launcherPath)) {
                $launcherProcess = $candidate
                $launcherHandle = $launcherProcess.SafeHandle
                $report.launcherPid = $launcherProcess.Id
            } else { $candidate.Dispose() }
        }
        $report.closeRequested = $gui.CloseMainWindow()
        if (-not $report.closeRequested) { throw 'Windows rejected the close request for the installed main window.' }
        $report.guiExited = $gui.WaitForExit(15000)
        if (-not $report.guiExited) { throw 'The installed GUI did not exit within 15 seconds of the close request.' }
        $report.guiExitCode = [HelloNeedleInstaller.ExitStatus]::Read($guiHandle)
        if ($launcherProcess) {
            $report.launcherExited = $launcherProcess.WaitForExit(10000)
            if (-not $report.launcherExited) { throw 'The installed launcher did not exit after the GUI closed.' }
            $report.launcherExitCode = [HelloNeedleInstaller.ExitStatus]::Read($launcherHandle)
            if ($report.launcherExitCode -ne $report.guiExitCode) {
                throw "GUI exit $($report.guiExitCode) differs from launcher exit $($report.launcherExitCode)."
            }
        }
        if ($report.guiExitCode -ne 0) {
            throw ('The installed GUI exited with native status 0x{0:X8}.' -f $report.guiExitCode)
        }
    } finally {
        $report | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $evidence 'start-menu-launch.json') -Encoding utf8
        if ($gui) {
            if (-not $gui.HasExited) { $gui.Kill($true); [void]$gui.WaitForExit(5000) }
            $gui.Dispose()
        }
        if ($launcherProcess) { $launcherProcess.Dispose() }
        if ($shellProcess) { $shellProcess.Dispose() }
    }
}

function Check-File([string]$Relative) {
    $p = Join-Path $directory $Relative
    if (-not [IO.File]::Exists($p) -or ([IO.FileInfo]$p).Length -eq 0) {
        throw "The installer omitted $Relative"
    }
    return $p
}

function Get-ProfileAssociations {
    # Search is read-only; installing it must not hijack existing reader/editor defaults.
    $snapshot = [ordered]@{}
    foreach ($ext in '.txt', '.pdf', '.doc', '.docx', '.odt') {
        $key = $account.OpenSubKey("Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\$ext\UserChoice")
        try { $snapshot[$ext] = if ($key) { $key.GetValue('ProgId') } } finally { if ($key) { $key.Dispose() } }
    }
    return ($snapshot | ConvertTo-Json -Compress)
}

function Check-CowIcon([string]$File, [string]$Label) {
    Add-Type -AssemblyName System.Drawing
    $ico = [Drawing.Icon]::ExtractAssociatedIcon($File)
    if (-not $ico) { throw "No shell icon is associated with $File" }
    $actual = $ico.ToBitmap()
    $master = [Drawing.Bitmap]::new((Join-Path $repo 'desktop/qt/helloneedle.png'))
    $reference = [Drawing.Bitmap]::new($actual.Width, $actual.Height)
    $painter = [Drawing.Graphics]::FromImage($reference)
    try {
        $painter.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $painter.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $painter.DrawImage($master, 0, 0, $actual.Width, $actual.Height)
        $total = 0.0
        $pink = 0
        for ($y = 0; $y -lt $actual.Height; $y++) {
            for ($x = 0; $x -lt $actual.Width; $x++) {
                $a = $actual.GetPixel($x, $y)
                $b = $reference.GetPixel($x, $y)
                if ($a.A -gt 128 -and $a.R -gt $a.G + 28 -and $a.R -gt $a.B + 20) { $pink++ }
                $total += [Math]::Abs([int]$a.A - [int]$b.A)
                foreach ($channel in 'R','G','B') {
                    $total += [Math]::Abs(([int]$a.$channel * $a.A - [int]$b.$channel * $b.A) / 255.0)
                }
            }
        }
        $mean = $total / ($actual.Width * $actual.Height * 4)
        if ($pink -lt 3 -or $mean -gt 25) { throw "Unexpected $Label artwork (error $mean, pink pixels $pink)" }
        $actual.Save((Join-Path $evidence "$Label-icon.png"), [Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $painter.Dispose(); $reference.Dispose(); $master.Dispose(); $actual.Dispose(); $ico.Dispose()
    }
}

function Check-Registration {
    $record = Get-Registration $account
    if (-not $record -or $record.DisplayVersion -ne $ExpectedVersion -or
        $record.DisplayName -notlike 'Hello Needle*' -or $record.Publisher -ne 'Alfredo Llaquet-Alsina' -or
        -not (Same-Path $record.InstallLocation $directory) -or
        $record.UninstallString -notmatch [regex]::Escape($directory + '\unins') -or
        $record.QuietUninstallString -notmatch '/SILENT|/VERYSILENT') {
        throw "Unexpected Add/Remove Programs registration: $($record | ConvertTo-Json -Compress)"
    }
    if (Get-Registration $machine) { throw 'An installation for the current user wrote a machine-wide uninstall record.' }
    return $record
}

function Get-Uninstaller {
    $found = @(Get-ChildItem -LiteralPath $directory -File -Filter 'unins*.exe' -ErrorAction SilentlyContinue)
    if ($found.Count -ne 1) { throw "Expected one uninstaller under $directory" }
    return $found[0].FullName
}

try {
    if ((Get-Registration $account) -or (Get-Registration $machine) -or
        (Test-Path -LiteralPath $directory) -or (Test-Path -LiteralPath $startLink) -or
        (Test-Path -LiteralPath $desktopLink)) {
        throw 'A Hello Needle installation or shortcut already exists; use a fresh test profile.'
    }
    [void][IO.Directory]::CreateDirectory($work)
    $registeredBefore = Get-ProfileAssociations
    foreach ($name in 'PATH','QT_PLUGIN_PATH','QT_QPA_PLATFORM_PLUGIN_PATH','QT_QPA_PLATFORMTHEME',
             'QT_QPA_PLATFORM','QT_STYLE_OVERRIDE','QT_CONF_PATH','QT_TRANSLATIONS_PATH','QTDIR',
             'HELLONEEDLE_UCRT64','HELLONEEDLE_WORKER','HELLO_NEEDLE_WORKER','ANTIWORDHOME',
             'POPPLER_DATADIR','FONTCONFIG_PATH','FONTCONFIG_FILE','MSYSTEM','MINGW_PREFIX','PYTHONPATH',
             'PYTHONHOME','PYTHONOPTIMIZE','QT_DEBUG_PLUGINS') {
        $settings[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $null)
    }
    $env:PATH = $systemPath
    [void](Invoke-Checked $pythonExe @($runtimeTest, 'prepare', '--documents', $documents, '--inventory', $inventory) 'prepare-documents')
    $silent = @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-')
    $stage = 'Install'
    $ownedInstallation = $true
    [void](Invoke-Checked $setup ($silent + @("/LOG=$(Join-Path $evidence 'inno-install.log')")) 'install' 300)
    $installRecord = Check-Registration
    if ($installRecord['Inno Setup: Language'] -notin 'english','spanish') {
        throw 'Unexpected default setup language.'
    }
    $launcher = Check-File 'Hello Needle.exe'
    $app = Check-File 'bin\helloneedle.exe'
    $worker = Check-File 'bin\helloneedle-worker.exe'
    $renderer = Check-File 'bin\pdftoppm.exe'
    $pdfText = Check-File 'bin\pdftotext.exe'
    foreach ($relative in 'bin\antiword.exe','bin\Qt6Core.dll','bin\Qt6Gui.dll',
        'bin\Qt6Widgets.dll','bin\qt.conf','plugins\platforms\qwindows.dll',
        'plugins\styles\qmodernwindowsstyle.dll','translations\qtbase_es.qm','share\antiword\UTF-8.txt',
        'share\antiword\fontnames','share\poppler\unicodeMap\EUC-JP','share\doc\helloneedle\LICENSE','share\doc\helloneedle\NOTICE.md',
        'share\doc\helloneedle\LICENSES\native\PACKAGES.json','share\doc\helloneedle\LICENSES\native\InnoSetup\LICENSE.txt') {
        [void](Check-File $relative)
    }
    if (-not (Get-ChildItem -LiteralPath (Join-Path $directory 'share\poppler') -Recurse -File | Select-Object -First 1)) {
        throw 'Missing bundled Poppler character maps.'
    }
    $langs = @(Get-ChildItem -LiteralPath (Join-Path $directory 'translations') -File -Filter 'qtbase_*.qm')
    if ($langs.Count -ne 1 -or $langs[0].Name -ne 'qtbase_es.qm') { throw 'Unexpected Qt language catalogs.' }
    $catalog = [Convert]::ToHexString([IO.File]::ReadAllBytes($langs[0].FullName))
    if (-not $catalog.StartsWith('3CB86418CAEF9C95CD211CBF60A1BDDD')) { throw 'Invalid Qt message catalog.' }
    foreach ($caption in 'Cancelar','Guardar','Abrir') {
        if (-not $catalog.Contains([Convert]::ToHexString([Text.Encoding]::BigEndianUnicode.GetBytes($caption)))) {
            throw "Qt's Spanish dialog catalog is missing $caption"
        }
    }
    $paths = Get-Content -LiteralPath (Join-Path $directory 'bin\qt.conf') -Raw
    foreach ($setting in '[Paths]', 'Prefix=..', 'Plugins=plugins', 'Translations=translations') {
        if ($setting -notin ($paths -split '\r?\n')) { throw "The installed qt.conf lost $setting" }
    }
    $inventoryJson = Get-Content -LiteralPath (Join-Path $directory 'share\doc\helloneedle\LICENSES\native\PACKAGES.json') -Raw
    [void]($inventoryJson | ConvertFrom-Json)
    foreach ($component in 'qt6-base','poppler','antiword') {
        if ($inventoryJson -notmatch [regex]::Escape($component)) { throw "Missing installed package notice for $component" }
    }
    foreach ($exe in $launcher,$app) {
        $v = [Diagnostics.FileVersionInfo]::GetVersionInfo($exe)
        if ($v.ProductName -ne 'Hello Needle' -or $v.ProductVersion -ne $ExpectedVersion -or
            $v.Comments -notmatch 'Alfredo Llaquet-Alsina' -or $v.Comments -notmatch 'Programmed by ChatGPT') {
            throw "Unexpected Explorer version or credits on $exe"
        }
    }
    Check-CowIcon $launcher 'launcher'
    Check-CowIcon $app 'desktop'
    Check-Shortcut $startLink $launcher
    if (Test-Path -LiteralPath $desktopLink) { throw 'The default install added an unsolicited desktop shortcut.' }
    if ((Get-ProfileAssociations) -ne $registeredBefore) { throw 'Installation changed a document association.' }
    $finished.Add('current-user install, registry, Start menu, icons, language and license inventory')

    $stage = 'Run installed application'
    $workerVersion = (Invoke-Checked $worker @('--version') 'worker-version').Trim()
    if ($workerVersion -ne "helloneedle-worker $ExpectedVersion") { throw "Unexpected worker version: $workerVersion" }
    $encodings = Invoke-Checked $pdfText @('-listenc') 'poppler-encodings'
    if (($encodings -split '\r?\n') -notcontains 'EUC-JP') { throw 'Installed Poppler could not find the bundled Japanese encoding map.' }
    Check-ShellLaunch $startLink $app
    [void](Invoke-Checked $launcher @('--smoke-test') 'installed-launcher' 90)
    [void](Invoke-Checked $app @('--smoke-test') 'installed-desktop' 90)
    [void](Invoke-Checked $pythonExe @($runtimeTest,'verify','--documents',$documents,'--inventory',$inventory,
        '--worker',$worker,'--pdf-renderer',$renderer,'--evidence',(Join-Path $evidence 'bundled-runtime.json')) 'bundled-runtime')
    [void](Invoke-Checked $pythonExe @((Join-Path $repo 'tests\desktop\worker_suite.py'),'--worker',$worker,'--require-doc') 'worker-suite' 180)
    $finished.Add('installed launcher, desktop and worker, PDF helpers and older Word decoder on system-only PATH')
    $installed.launcher = $launcher
    $installed.worker = $worker
    $installed.version = $installRecord.DisplayVersion

    if ($tester) {
        $stage = 'Native GUI matrix'
        $testDll = Join-Path (Split-Path -Parent $tester) 'Qt6Test.dll'
        if (-not [IO.File]::Exists($testDll)) {
            throw 'The separate QA artifact must include its Qt6Test.dll.'
        }
        # Only the external test harness needs Qt from the installed program.
        # The two production process smoke tests above run independently first.
        $driverDirectory = Join-Path $work 'Graphical tests'
        [void][IO.Directory]::CreateDirectory($driverDirectory)
        $stagedTester = Join-Path $driverDirectory 'helloneedle-gui-tests.exe'
        Copy-Item -LiteralPath $tester -Destination $stagedTester
        Copy-Item -LiteralPath $testDll -Destination $driverDirectory
        $driverConf = @('[Paths]',('Prefix=' + $directory.Replace('\','/')), 'Plugins=plugins','Translations=translations')
        [IO.File]::WriteAllLines((Join-Path $driverDirectory 'qt.conf'), $driverConf, [Text.UTF8Encoding]::new($false))
        $env:PATH = "$(Join-Path $directory 'bin');$systemPath"
        $env:QT_PLUGIN_PATH = Join-Path $directory 'plugins'
        try {
            [void](Invoke-Checked $pythonExe @((Join-Path $repo 'tests\desktop\gui_matrix.py'),
                '--tester',$stagedTester,'--worker',$worker,'--output',(Join-Path $evidence 'gui')) 'native-gui-matrix' 1500)
        } finally { $env:PATH = $systemPath; Remove-Item Env:\QT_PLUGIN_PATH -ErrorAction SilentlyContinue }
        $finished.Add('native GUI scenarios using separate test driver and installed desktop runtime')
    }
    $stage = 'Repair and optional shortcut'
    [void](Invoke-Checked $setup ($silent + @('/LANG=spanish','/TASKS=desktopicon',"/LOG=$(Join-Path $evidence 'inno-repair.log')")) 'repair' 300)
    $repairRecord = Check-Registration
    if ($repairRecord['Inno Setup: Language'] -ne 'spanish') { throw 'Setup did not switch to the Spanish translation for repair.' }
    Check-Shortcut $startLink $launcher
    Check-Shortcut $desktopLink $launcher
    [void](Invoke-Checked $launcher @('--smoke-test') 'reinstalled-launcher' 90)
    $finished.Add('same-version reinstall in Spanish and optional desktop shortcut')

    $stage = 'Uninstall'
    $uninstaller = Get-Uninstaller
    [void](Invoke-Checked $uninstaller ($silent + @("/LOG=$(Join-Path $evidence 'inno-uninstall.log')")) 'uninstall' 300)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline -and (Test-Path -LiteralPath $directory)) { Start-Sleep -Milliseconds 200 }
    if ((Get-Registration $account) -or (Get-Registration $machine) -or
        (Test-Path -LiteralPath $directory) -or (Test-Path -LiteralPath $startLink) -or
        (Test-Path -LiteralPath $desktopLink)) { throw 'Uninstall left installed files, registry registration or Shell shortcuts.' }
    $ownedInstallation = $false
    [void](Invoke-Checked $pythonExe @($runtimeTest,'unchanged','--documents',$documents,'--inventory',$inventory) 'preserved-originals')
    if ((Get-ProfileAssociations) -ne $registeredBefore) { throw 'Uninstall changed a document association.' }
    $finished.Add('uninstall, shortcut and registry cleanup, bit-for-bit preservation of original documents')
    $stage = 'Complete'
    Write-Output "Hello Needle $ExpectedVersion Windows installer passed."
} finally {
    $cleanupError = $null
    if ($ownedInstallation -and (Test-Path -LiteralPath $directory)) {
        $cleanup = @(Get-ChildItem -LiteralPath $directory -File -Filter 'unins*.exe' -ErrorAction SilentlyContinue)
        if ($cleanup.Count -eq 1) {
            try {
                [void](Invoke-Checked $cleanup[0].FullName @('/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART') 'cleanup-uninstall' 300)
            } catch { $cleanupError = $_.Exception.Message }
        }
    }
    [ordered]@{version=$ExpectedVersion; platform=[Environment]::OSVersion.VersionString;
        state=$stage; completed=$finished.ToArray(); install=$installed; systemPath=$systemPath;
        guiMatrix=[bool]$tester; cleanupError=$cleanupError} | ConvertTo-Json -Depth 4 |
            Set-Content -LiteralPath (Join-Path $evidence 'installer-result.json') -Encoding utf8
    foreach ($name in $settings.Keys) { [Environment]::SetEnvironmentVariable($name, $settings[$name]) }
    $account.Dispose(); $machine.Dispose()
    if (Test-Path -LiteralPath $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
