# The C tools: the data-set runner over the C library (dataset), with CRE_QUICKJS the same runner over QuickJS's
# libregexp (dataset-quickjs), and with CRE_BENCH the Industry benchmark harness (industry, POSIX only) over the C
# library, libregexp and PCRE2. Each is a C program that includes corvus_regex_ecma262.h and links the library as a
# consumer does:
#
#   cargo build --release -p corvus-regex-ecma262-capi
#   cmake -S tools/c -B build/tools -DCRE_LIB_DIR=$PWD/target/release [-DCRE_QUICKJS=ON] [-DCRE_BENCH=ON]
#   cmake --build build/tools --config Release
#   build/tools/bin/dataset <ecma262-regex-test-data>/data wrong-cases-c.tsv
#
# With CRE_PREFIX (an unpacked release archive) instead of CRE_LIB_DIR, the library is the archive's, found through
# its CMake package. QuickJS is quickjs-ng and PCRE2 is PCRE2, each at a pinned release (QUICKJS_URL, PCRE2_URL),
# downloaded at configure time.
cmake_minimum_required(VERSION 3.24)
project(corvus_regex_ecma262_tools C)

set(CRE_LIB_DIR "" CACHE PATH "The directory with the library cargo built (target/release)")
set(CRE_PREFIX "" CACHE PATH "An unpacked release archive, used through its CMake package")
option(CRE_QUICKJS "Build the runner over QuickJS's libregexp too" OFF)
option(CRE_BENCH "Build the Industry benchmark harness (with QuickJS and PCRE2)" OFF)
if(CRE_BENCH)
  set(CRE_QUICKJS ON)
endif()

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
  set(CMAKE_BUILD_TYPE Release)
endif()
find_package(Threads REQUIRED)
# Every program in build/bin, whatever the generator (the generator expression keeps a multi-config generator from
# adding Release/).
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin/$<0:>")

if(MSVC)
  set(warnings /W4 /WX /wd4996)
else()
  set(warnings -Wall -Wextra -Werror -Wno-unused-parameter)
endif()

# The C library, from cargo's output or from a release archive.
if(CRE_PREFIX)
  find_package(corvus_regex_ecma262 REQUIRED PATHS "${CRE_PREFIX}" NO_DEFAULT_PATH)
  set(cre_library corvus_regex_ecma262::corvus_regex_ecma262)
else()
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
  set_target_properties(corvus_regex_ecma262 PROPERTIES
    INTERFACE_INCLUDE_DIRECTORIES "${CMAKE_CURRENT_SOURCE_DIR}/../../corvus-regex-ecma262-capi/include")
  set(cre_library corvus_regex_ecma262)
endif()

add_library(cre_tools_json STATIC json.c)
target_compile_options(cre_tools_json PRIVATE ${warnings})

add_executable(dataset dataset.c)
target_link_libraries(dataset PRIVATE cre_tools_json ${cre_library} Threads::Threads)
target_compile_options(dataset PRIVATE ${warnings})
if(NOT WIN32)
  get_target_property(cre_location ${cre_library} IMPORTED_LOCATION)
  if(cre_location)
    get_filename_component(cre_location_dir "${cre_location}" DIRECTORY)
    set_target_properties(dataset PROPERTIES BUILD_RPATH "${cre_location_dir}")
  endif()
else()
  # The DLL is found next to the program.
  add_custom_command(TARGET dataset POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different $<TARGET_FILE:${cre_library}> $<TARGET_FILE_DIR:dataset>)
endif()

if(CRE_QUICKJS)
  include(FetchContent)
  # quickjs-ng, the maintained fork of QuickJS: tagged releases, and builds with MSVC as well as GCC and Clang.
  set(QUICKJS_VERSION "0.17.0")
  set(QUICKJS_URL "https://github.com/quickjs-ng/quickjs/archive/refs/tags/v${QUICKJS_VERSION}.tar.gz")
  set(QUICKJS_SHA256 "559bc4c420475e55c7ab4510adbc562f55d7524d75e8e89d79ce4bb02f5687d9")
  # Only its sources are wanted, not its own build (SOURCE_SUBDIR names a directory it does not have).
  FetchContent_Declare(quickjs URL "${QUICKJS_URL}" URL_HASH SHA256=${QUICKJS_SHA256} DOWNLOAD_EXTRACT_TIMESTAMP TRUE
    SOURCE_SUBDIR sources-only)
  FetchContent_MakeAvailable(quickjs)
  # libregexp alone, as quickjs-ng's own lre-test builds it: the engine and its Unicode tables.
  add_library(quickjs_regexp STATIC "${quickjs_SOURCE_DIR}/libregexp.c" "${quickjs_SOURCE_DIR}/libunicode.c")
  target_include_directories(quickjs_regexp PUBLIC "${quickjs_SOURCE_DIR}")
  if(MSVC)
    target_compile_options(quickjs_regexp PRIVATE /experimental:c11atomics /wd4244 /wd4018)
  else()
    target_compile_options(quickjs_regexp PRIVATE -funsigned-char -w)
  endif()
  add_executable(dataset-quickjs dataset.c)
  target_compile_definitions(dataset-quickjs PRIVATE DATASET_QUICKJS)
  target_link_libraries(dataset-quickjs PRIVATE cre_tools_json quickjs_regexp Threads::Threads)
  target_compile_options(dataset-quickjs PRIVATE ${warnings})
endif()

if(CRE_BENCH)
  # PCRE2 with its JIT, from the release's own archive (which carries the JIT's sources).
  set(PCRE2_URL "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.49/pcre2-10.49.tar.gz")
  set(PCRE2_SHA256 "929f0b20e62879252a15886b06c89f1edef61a363cbd5826fb041080a5e557ae")
  set(PCRE2_SUPPORT_JIT ON CACHE BOOL "" FORCE)
  set(PCRE2_BUILD_TESTS OFF CACHE BOOL "" FORCE)
  set(PCRE2_BUILD_PCRE2GREP OFF CACHE BOOL "" FORCE)
  set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
  set(BUILD_STATIC_LIBS ON CACHE BOOL "" FORCE)
  FetchContent_Declare(pcre2 URL "${PCRE2_URL}" URL_HASH SHA256=${PCRE2_SHA256} DOWNLOAD_EXTRACT_TIMESTAMP TRUE)
  FetchContent_MakeAvailable(pcre2)
  add_executable(industry industry.c)
  target_compile_definitions(industry PRIVATE "QUICKJS_VERSION=\"${QUICKJS_VERSION} (quickjs-ng)\"")
  target_link_libraries(industry PRIVATE cre_tools_json quickjs_regexp pcre2-8-static ${cre_library} m)
  target_compile_options(industry PRIVATE ${warnings})
  get_target_property(cre_location ${cre_library} IMPORTED_LOCATION)
  if(cre_location)
    get_filename_component(cre_location_dir "${cre_location}" DIRECTORY)
    set_target_properties(industry PROPERTIES BUILD_RPATH "${cre_location_dir}")
  endif()
endif()
