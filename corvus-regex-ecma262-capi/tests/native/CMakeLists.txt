# The C smoke test, built as a consumer builds it, in either of two ways:
#
# - against a package from package.ps1 (a release archive, unpacked), through its CMake package, once with the shared
#   library and once with the static one (and on MSVC with the static library for /MT):
#
#     cmake -S tests/native -B build -DCMAKE_PREFIX_PATH=<package directory> [-DCRE_SANITIZE=ON]
#     cmake --build build --config Release && ctest --test-dir build -C Release --output-on-failure
#
# - against the shared library cargo builds:
#
#     cargo build --release -p corvus-regex-ecma262-capi
#     cmake -S corvus-regex-ecma262-capi/tests/native -B build/native -DCRE_LIB_DIR=$PWD/target/release
#     cmake --build build/native --config Release && ctest --test-dir build/native -C Release --output-on-failure
cmake_minimum_required(VERSION 3.16)
project(corvus_regex_ecma262_smoke C)

set(CRE_LIB_DIR "" CACHE PATH "The directory with the library cargo built (target/release); empty for a package")
option(CRE_SANITIZE "Build with AddressSanitizer and UndefinedBehaviorSanitizer (Clang or GCC)" OFF)

if(MSVC)
  set(warnings /W4 /WX)
else()
  set(warnings -Wall -Wextra -Wpedantic -Werror)
endif()

if(CRE_LIB_DIR)
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
  set(kinds cargo)
else()
  find_package(corvus_regex_ecma262 0.1 CONFIG REQUIRED)
  set(kinds shared static)
  if(TARGET corvus_regex_ecma262::corvus_regex_ecma262_static_mt)
    # MSVC: the static library for the static C runtime, linked from a test built with /MT.
    list(APPEND kinds static_mt)
  endif()
endif()

enable_testing()
foreach(kind ${kinds})
  if(kind STREQUAL "cargo")
    set(library corvus_regex_ecma262)
    set(test smoke)
  elseif(kind STREQUAL "shared")
    set(library corvus_regex_ecma262::corvus_regex_ecma262)
    set(test smoke_shared)
  elseif(kind STREQUAL "static")
    set(library corvus_regex_ecma262::corvus_regex_ecma262_static)
    set(test smoke_static)
  else()
    set(library corvus_regex_ecma262::corvus_regex_ecma262_static_mt)
    set(test smoke_static_mt)
  endif()
  add_executable(${test} smoke.c)
  # The header is C99; MSVC has no C99 mode, so C11 there.
  if(MSVC)
    set_target_properties(${test} PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
  else()
    set_target_properties(${test} PROPERTIES C_STANDARD 99 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
  endif()
  target_link_libraries(${test} PRIVATE ${library})
  target_compile_options(${test} PRIVATE ${warnings})
  if(kind STREQUAL "static_mt")
    set_target_properties(${test} PROPERTIES MSVC_RUNTIME_LIBRARY "MultiThreaded$<$<CONFIG:Debug>:Debug>")
  endif()
  if(kind STREQUAL "cargo" AND NOT WIN32)
    set_target_properties(${test} PROPERTIES BUILD_RPATH "${CRE_LIB_DIR}")
  endif()
  if(CRE_SANITIZE)
    target_compile_options(${test} PRIVATE -fsanitize=address,undefined -fno-omit-frame-pointer)
    target_link_options(${test} PRIVATE -fsanitize=address,undefined)
  endif()
  if(CRE_SANITIZE AND CMAKE_SYSTEM_NAME STREQUAL "Linux")
    # Older AddressSanitizer runtimes crash at random on kernels that randomise more address bits (vm.mmap_rnd_bits
    # above 28) before reporting anything; without address-space randomisation they run.
    add_test(NAME ${test} COMMAND setarch ${CMAKE_SYSTEM_PROCESSOR} -R $<TARGET_FILE:${test}>)
  else()
    add_test(NAME ${test} COMMAND ${test})
  endif()
  if(WIN32 AND (kind STREQUAL "cargo" OR kind STREQUAL "shared"))
    # The DLL is found next to the test.
    add_custom_command(TARGET ${test} POST_BUILD
      COMMAND ${CMAKE_COMMAND} -E copy_if_different $<TARGET_FILE:${library}> $<TARGET_FILE_DIR:${test}>)
  endif()
endforeach()
