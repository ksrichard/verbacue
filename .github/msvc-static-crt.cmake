# Makes every CMake build in the Windows CI jobs use the static C runtime (/MT), the
# same as the Rust code (+crt-static). ggml turns on policy CMP0091, so CMake adds /MD
# itself and the /MT flag that the build scripts pass has no effect.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
