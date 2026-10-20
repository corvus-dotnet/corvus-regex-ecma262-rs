# The C smoke test, built against the shared library cargo builds, as a consumer would link it:
#
#   cargo build --release -p corvus-regex-ecma262-capi
#   cmake -S corvus-regex-ecma262-capi/tests/native -B build/native -DCRE_LIB_DIR=$PWD/target/release
#   cmake --build build/native --config Release && ctest --test-dir build/native -C Release --output-on-failure
cmake_minimum_required(VERSION 3.16)
project(corvus_regex_ecma262_smoke C)

set(CRE_LIB_DIR "" CACHE PATH "The directory with the built library (target/release)")
set(CRE_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../../include")

add_library(corvus_regex_ecma262 SHARED IMPORTED)
if(WIN32)
  set_target_properties(corvus_regex_ecma262 PROPERTIES
    IMPORTED_LOCATION "${CRE_LIB_DIR}/corvus_regex_ecma262.dll"
    IMPORTED_IMPLIB "${CRE_LIB_DIR}/corvus_regex_ecma262.dll.lib")
elseif(APPLE)
  set_target_properties(corvus_regex_ecma262 PROPERTIES IMPORTED_LOCATION "${CRE_LIB_DIR}/libcorvus_regex_ecma262.dylib")
else()
  set_target_properties(corvus_regex_ecma262 PROPERTIES IMPORTED_LOCATION "${CRE_LIB_DIR}/libcorvus_regex_ecma262.so")
endif()
set_target_properties(corvus_regex_ecma262 PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "${CRE_INCLUDE_DIR}")

if(MSVC)
  set(warnings /W4 /WX)
else()
  set(warnings -Wall -Wextra -Wpedantic -Werror)
endif()

enable_testing()
add_executable(smoke smoke.c)
# The header is C99; MSVC has no C99 mode, so C11 there.
if(MSVC)
  set_target_properties(smoke PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
else()
  set_target_properties(smoke PROPERTIES C_STANDARD 99 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
endif()
target_link_libraries(smoke PRIVATE corvus_regex_ecma262)
target_compile_options(smoke PRIVATE ${warnings})
if(NOT WIN32)
  set_target_properties(smoke PROPERTIES BUILD_RPATH "${CRE_LIB_DIR}")
endif()
add_test(NAME smoke COMMAND smoke)
if(WIN32)
  # The DLL is found next to the test.
  add_custom_command(TARGET smoke POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different "${CRE_LIB_DIR}/corvus_regex_ecma262.dll" $<TARGET_FILE_DIR:smoke>)
endif()
