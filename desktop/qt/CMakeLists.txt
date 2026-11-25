# SPDX-License-Identifier: GPL-3.0-or-later
find_package(Qt6 6.4 REQUIRED COMPONENTS Widgets Svg)
add_library(helloneedle-desktop STATIC
    application.cpp language.cpp runtime.cpp workerclient.cpp mainwindow.cpp resultnavigation.cpp searchstatus.cpp buttonicons.cpp
    previewpane.cpp pdfpane.cpp performancesettings.cpp credits.cpp)
target_include_directories(helloneedle-desktop PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
target_compile_features(helloneedle-desktop PUBLIC cxx_std_17)
target_link_libraries(helloneedle-desktop PUBLIC Qt6::Widgets PRIVATE Qt6::Svg)
target_compile_definitions(helloneedle-desktop PRIVATE
    HN_VERSION="${PROJECT_VERSION}")
if(MSVC)
    target_compile_options(helloneedle-desktop PRIVATE /utf-8)
endif()

qt_add_resources(helloneedle-desktop hello_needle_art
    PREFIX "/art" FILES helloneedle.png
    icons/folder-open.svg icons/search.svg icons/circle-stop.svg
    icons/sliders-horizontal.svg icons/list.svg icons/external-link.svg
    icons/chevrons-down.svg icons/chevron-down.svg icons/chevron-up.svg icons/chevrons-up.svg)

# Link the texts into the application as well as installing the distribution's
# notices. The license dialog is available even in an unpackaged build tree.
get_filename_component(repo_root "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
file(GLOB_RECURSE component_licenses CONFIGURE_DEPENDS "${repo_root}/LICENSES/*")
qt_add_resources(helloneedle-desktop hello_needle_licenses
    PREFIX "/licenses" BASE "${repo_root}"
    FILES "${repo_root}/LICENSE" "${repo_root}/NOTICE.md" ${component_licenses})

qt_add_executable(helloneedle main.cpp)
target_link_libraries(helloneedle PRIVATE helloneedle-desktop)
