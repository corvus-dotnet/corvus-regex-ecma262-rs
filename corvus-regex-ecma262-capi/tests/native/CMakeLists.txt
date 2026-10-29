# The C smoke test and the C programs of docs/getting-started-c.md, built as a consumer builds them, in either of two
# ways:
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
  find_package(corvus_regex_ecma262 0.0 CONFIG REQUIRED)
  set(kinds shared static)
  if(TARGET corvus_regex_ecma262::corvus_regex_ecma262_static_mt)
    # MSVC: the static library for the static C runtime, linked from a test built with /MT.
    list(APPEND kinds static_mt)
  endif()
endif()

# The C programs of the getting-started page the documentation site publishes (docs/getting-started-c.md): each block
# of C with a main is a program of its own, built and run as the smoke test is, so that the page shows only code that works.
set(CRE_GETTING_STARTED "${CMAKE_CURRENT_SOURCE_DIR}/../../../docs/getting-started-c.md" CACHE FILEPATH
  "The getting-started page whose C programs are built and run as tests")
set(samples)
if(EXISTS "${CRE_GETTING_STARTED}")
  file(READ "${CRE_GETTING_STARTED}" doc)
  set(n 0)
  while(TRUE)
    string(FIND "${doc}" "```c\n" begin)
    if(begin EQUAL -1)
      break()
    endif()
    math(EXPR begin "${begin} + 5")
    string(SUBSTRING "${doc}" ${begin} -1 doc)
    string(FIND "${doc}" "\n```" end)
    string(SUBSTRING "${doc}" 0 ${end} code)
    math(EXPR end "${end} + 4")
    string(SUBSTRING "${doc}" ${end} -1 doc)
    # A block that is not a whole program, such as the #include of Setup, is not built.
    string(FIND "${code}" "int main(" has_main)
    if(has_main EQUAL -1)
      continue()
    endif()
    math(EXPR n "${n} + 1")
    file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/getting_started_${n}.c" "${code}\n")
    list(APPEND samples "getting_started_${n}")
  endwhile()
endif()

enable_testing()
foreach(kind ${kinds})
  if(kind STREQUAL "cargo")
    set(library corvus_regex_ecma262)
    set(suffix "")
  elseif(kind STREQUAL "shared")
    set(library corvus_regex_ecma262::corvus_regex_ecma262)
    set(suffix _shared)
  elseif(kind STREQUAL "static")
    set(library corvus_regex_ecma262::corvus_regex_ecma262_static)
    set(suffix _static)
  else()
    set(library corvus_regex_ecma262::corvus_regex_ecma262_static_mt)
    set(suffix _static_mt)
  endif()
  set(programs "smoke=${CMAKE_CURRENT_SOURCE_DIR}/smoke.c")
  foreach(sample ${samples})
    list(APPEND programs "${sample}=${CMAKE_CURRENT_BINARY_DIR}/${sample}.c")
  endforeach()
  foreach(program ${programs})
    string(REPLACE "=" ";" parts "${program}")
    list(GET parts 0 name)
    list(GET parts 1 source)
    set(test ${name}${suffix})
    add_executable(${test} ${source})
    # The header is C99; MSVC has no C99 mode, so C11 there.
    if(MSVC)
      set_target_properties(${test} PROPERTIES C_STANDARD 11 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
    else()
      set_target_properties(${test} PROPERTIES C_STANDARD 99 C_STANDARD_REQUIRED ON C_EXTENSIONS OFF)
    endif()
    target_link_libraries(${test} PRIVATE ${library})
    target_compile_options(${test} PRIVATE ${warnings})
    if(NOT name STREQUAL "smoke")
      # The page's programs check with assert, which a Release build would otherwise compile away.
      if(MSVC)
        target_compile_options(${test} PRIVATE /UNDEBUG)
      else()
        target_compile_options(${test} PRIVATE -UNDEBUG)
      endif()
    endif()
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
endforeach()
