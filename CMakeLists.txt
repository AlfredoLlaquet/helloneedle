# SPDX-License-Identifier: GPL-3.0-or-later
cmake_minimum_required(VERSION 3.21)
project(HelloNeedle VERSION 1.0.0 LANGUAGES CXX)

include(GNUInstallDirs)
include(CTest)
find_package(Qt6 6.4 REQUIRED COMPONENTS Widgets)
qt_standard_project_setup()
find_package(Python3 3.9 REQUIRED COMPONENTS Interpreter)
find_program(CARGO_EXECUTABLE cargo REQUIRED)

if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin")
if(APPLE)
    # Retain the system package manager's Qt library paths in the installed app.
    set(CMAKE_INSTALL_RPATH_USE_LINK_PATH TRUE)
endif()
add_subdirectory(desktop/qt)

set(icon_master "${PROJECT_SOURCE_DIR}/desktop/qt/helloneedle.png")
set(icon_dir "${PROJECT_BINARY_DIR}/icons")
set(icon_sizes 16 22 24 32 48 64 96 128 256 512 1024)
set(icon_files "${icon_dir}/helloneedle.ico" "${icon_dir}/helloneedle.icns")
foreach(size IN LISTS icon_sizes)
    list(APPEND icon_files "${icon_dir}/${size}.png")
endforeach()
add_executable(helloneedle-icon-sizes EXCLUDE_FROM_ALL packaging/scale-icon.cpp)
set_target_properties(helloneedle-icon-sizes PROPERTIES AUTOMOC OFF AUTOUIC OFF)
target_link_libraries(helloneedle-icon-sizes PRIVATE Qt6::Gui)
add_custom_command(OUTPUT ${icon_files}
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${icon_dir}"
    COMMAND helloneedle-icon-sizes "${icon_master}" "${icon_dir}" ${icon_sizes}
    COMMAND "${Python3_EXECUTABLE}" "${PROJECT_SOURCE_DIR}/packaging/icons.py" "${icon_dir}"
    DEPENDS helloneedle-icon-sizes "${icon_master}" packaging/icons.py
    VERBATIM)
add_custom_target(helloneedle-icons ALL DEPENDS ${icon_files})
add_dependencies(helloneedle helloneedle-icons)

set(worker_name "helloneedle-worker${CMAKE_EXECUTABLE_SUFFIX}")
set(worker_dir "${CMAKE_RUNTIME_OUTPUT_DIRECTORY}")
if(CMAKE_CONFIGURATION_TYPES)
    string(APPEND worker_dir "/$<CONFIG>")
endif()
set(HELLO_NEEDLE_WORKER_EXECUTABLE "${worker_dir}/${worker_name}")
set(cargo_dir "${CMAKE_BINARY_DIR}/cargo")
set(cargo_profile "$<IF:$<CONFIG:Debug>,dev,release>")
set(cargo_artifact_dir "$<IF:$<CONFIG:Debug>,debug,release>")
add_custom_target(helloneedle-worker ALL
    COMMAND "${CARGO_EXECUTABLE}" build --locked --profile "${cargo_profile}"
            --manifest-path "${CMAKE_CURRENT_SOURCE_DIR}/bridge/Cargo.toml"
            --target-dir "${cargo_dir}"
    COMMAND "${CMAKE_COMMAND}" -E make_directory "${worker_dir}"
    COMMAND "${CMAKE_COMMAND}" -E copy_if_different
            "${cargo_dir}/${cargo_artifact_dir}/${worker_name}" "${HELLO_NEEDLE_WORKER_EXECUTABLE}"
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    VERBATIM USES_TERMINAL)

set(rust_notices "${CMAKE_BINARY_DIR}/rust-licenses")
add_custom_command(
    OUTPUT "${rust_notices}/DEPENDENCIES.md"
    COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/tools/source_bundle.py"
            --notices-only "${rust_notices}"
    DEPENDS tools/source_bundle.py bridge/Cargo.lock bridge/Cargo.toml
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}"
    VERBATIM)
add_custom_target(helloneedle-notices ALL DEPENDS "${rust_notices}/DEPENDENCIES.md")
add_dependencies(helloneedle helloneedle-worker helloneedle-notices)

if(APPLE)
    # Keep the binary named helloneedle inside the human-readable Finder bundle.
    set_target_properties(helloneedle PROPERTIES MACOSX_BUNDLE FALSE)
    set(contents "Hello Needle.app/Contents")
    set(app_bindir "${contents}/MacOS")
    set(app_docdir "${contents}/Resources/licenses")
    configure_file(cmake/Info.plist.in Info.plist @ONLY)
    install(FILES "${CMAKE_BINARY_DIR}/Info.plist" DESTINATION "${contents}")
    install(FILES "${icon_dir}/helloneedle.icns" DESTINATION "${contents}/Resources")
    install(FILES packaging/MACOS.txt DESTINATION "${contents}/Resources")
else()
    set(app_bindir "${CMAKE_INSTALL_BINDIR}")
    set(app_docdir "${CMAKE_INSTALL_DATADIR}/doc/helloneedle")
    if(WIN32)
        enable_language(C RC)
        set_target_properties(helloneedle PROPERTIES WIN32_EXECUTABLE TRUE)
        configure_file(cmake/windows.rc.in windows.rc @ONLY)
        target_sources(helloneedle PRIVATE "${CMAKE_BINARY_DIR}/windows.rc")
        # This small launcher can display installation help before Qt is available.
        add_executable(helloneedle-launcher WIN32 packaging/windows-launcher.c "${CMAKE_BINARY_DIR}/windows.rc")
        set_target_properties(helloneedle-launcher PROPERTIES OUTPUT_NAME "Hello Needle" AUTOMOC OFF AUTOUIC OFF)
        target_compile_definitions(helloneedle-launcher PRIVATE $<$<COMPILE_LANGUAGE:RC>:HELLO_NEEDLE_LAUNCHER>)
        target_link_libraries(helloneedle-launcher PRIVATE user32)
        add_dependencies(helloneedle-launcher helloneedle-icons)
        set_source_files_properties("${CMAKE_BINARY_DIR}/windows.rc" TARGET_DIRECTORY helloneedle helloneedle-launcher
                PROPERTIES OBJECT_DEPENDS "${icon_dir}/helloneedle.ico")
        if(MINGW)
            target_compile_options(helloneedle-launcher PRIVATE $<$<COMPILE_LANGUAGE:C>:-finput-charset=UTF-8>)
            target_link_options(helloneedle-launcher PRIVATE -municode)
        elseif(MSVC)
            target_compile_options(helloneedle-launcher PRIVATE $<$<COMPILE_LANGUAGE:C>:/utf-8>)
        endif()
        install(TARGETS helloneedle-launcher RUNTIME DESTINATION ".")
        install(FILES packaging/WINDOWS.txt DESTINATION ".")
    else()
        install(FILES packaging/org.thecrumbs.HelloNeedle.desktop
                DESTINATION "${CMAKE_INSTALL_DATADIR}/applications")
        foreach(size IN LISTS icon_sizes)
            install(FILES "${icon_dir}/${size}.png" RENAME helloneedle.png
                    DESTINATION "${CMAKE_INSTALL_DATADIR}/icons/hicolor/${size}x${size}/apps")
        endforeach()
    endif()
endif()

# The external Qt libraries and document converters come from the platform's
# package manager. The statically linked worker crates have their own notices.
set(qt_version "${Qt6_VERSION}")
configure_file(cmake/NATIVE-COMPONENTS.md.in NATIVE-COMPONENTS.md @ONLY)
install(TARGETS helloneedle RUNTIME DESTINATION "${app_bindir}")
install(PROGRAMS "${HELLO_NEEDLE_WORKER_EXECUTABLE}" DESTINATION "${app_bindir}")
install(FILES LICENSE NOTICE.md "${CMAKE_BINARY_DIR}/NATIVE-COMPONENTS.md"
        DESTINATION "${app_docdir}")
install(DIRECTORY LICENSES/ DESTINATION "${app_docdir}/LICENSES")
install(DIRECTORY "${rust_notices}/" DESTINATION "${app_docdir}/LICENSES/rust")

if(BUILD_TESTING)
    add_subdirectory(tests/desktop)
endif()
