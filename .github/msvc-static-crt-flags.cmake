# Loaded by msvc-static-crt.cmake after CMake sets its default compiler flags.
foreach(lang C CXX)
  foreach(var FLAGS_INIT FLAGS_DEBUG_INIT FLAGS_RELEASE_INIT FLAGS_RELWITHDEBINFO_INIT
              FLAGS_MINSIZEREL_INIT)
    string(REPLACE "/MD" "/MT" CMAKE_${lang}_${var} "${CMAKE_${lang}_${var}}")
  endforeach()
endforeach()
