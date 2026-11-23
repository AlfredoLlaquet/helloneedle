# SPDX-License-Identifier: GPL-3.0-or-later
find_package(Qt6 6.4 REQUIRED COMPONENTS Widgets)
add_library(helloneedle-desktop STATIC
    application.cpp language.cpp runtime.cpp workerclient.cpp mainwindow.cpp
    previewpane.cpp pdfpane.cpp credits.cpp)
target_include_directories(helloneedle-desktop PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}")
target_compile_features(helloneedle-desktop PUBLIC cxx_std_17)
target_link_libraries(helloneedle-desktop PUBLIC Qt6::Widgets)
target_compile_definitions(helloneedle-desktop PRIVATE
    HN_VERSION="${PROJECT_VERSION}")
if(MSVC)
    target_compile_options(helloneedle-desktop PRIVATE /utf-8)
endif()

qt_add_resources(helloneedle-desktop hello_needle_art
    PREFIX "/art" FILES helloneedle.png)

# Link the texts into the application as well as installing the distribution's
# notices. The license dialog is available even in an unpackaged build tree.
get_filename_component(repo_root "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
file(GLOB_RECURSE component_licenses CONFIGURE_DEPENDS "${repo_root}/LICENSES/*")
qt_add_resources(helloneedle-desktop hello_needle_licenses
    PREFIX "/licenses" BASE "${repo_root}"
    FILES "${repo_root}/LICENSE" "${repo_root}/NOTICE.md" ${component_licenses})

qt_add_executable(helloneedle main.cpp)
target_link_libraries(helloneedle PRIVATE helloneedle-desktop)
