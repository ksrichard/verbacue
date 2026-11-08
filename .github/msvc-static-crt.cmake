# Makes every CMake build in the Windows CI jobs use the static C runtime (/MT), the
# same as the Rust code (+crt-static).
# - Projects with policy CMP0091 on (llama.cpp) read CMAKE_MSVC_RUNTIME_LIBRARY.
# - Projects with it off (whisper.cpp asks for CMake 3.5) take /MD from the default
#   flags of each build type. The rules override file changes those flags.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
set(CMAKE_USER_MAKE_RULES_OVERRIDE "${CMAKE_CURRENT_LIST_DIR}/msvc-static-crt-flags.cmake")
