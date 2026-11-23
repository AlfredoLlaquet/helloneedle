find_package(Qt6 REQUIRED COMPONENTS Test)
find_package(Python3 REQUIRED COMPONENTS Interpreter)

add_executable(helloneedle-gui-tests gui_tests.cpp gui_appearance.cpp)
set_target_properties(helloneedle-gui-tests PROPERTIES AUTOMOC ON)
target_compile_features(helloneedle-gui-tests PRIVATE cxx_std_17)
target_link_libraries(helloneedle-gui-tests PRIVATE helloneedle-desktop Qt6::Test)
if(APPLE)
    enable_language(OBJCXX)
    target_sources(helloneedle-gui-tests PRIVATE picker_mac.mm)
    target_link_libraries(helloneedle-gui-tests PRIVATE "-framework AppKit")
elseif(WIN32)
    target_sources(helloneedle-gui-tests PRIVATE picker_win.cpp)
    target_link_libraries(helloneedle-gui-tests PRIVATE user32)
endif()

add_test(NAME native-desktop-gui
    COMMAND "${Python3_EXECUTABLE}" "${CMAKE_CURRENT_SOURCE_DIR}/gui_matrix.py"
        --tester "$<TARGET_FILE:helloneedle-gui-tests>"
        --worker "${HELLO_NEEDLE_WORKER_EXECUTABLE}"
        --output "${CMAKE_BINARY_DIR}/gui-evidence")
set_tests_properties(native-desktop-gui PROPERTIES TIMEOUT 900 LABELS "desktop;native")
