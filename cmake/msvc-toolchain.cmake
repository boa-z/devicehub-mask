# Avoid shared compiler PDB failures in MSVC CMake dependencies. Embedded debug
# information keeps debug builds parallel-safe and leaves release code unchanged.
if(CMAKE_GENERATOR MATCHES "Visual Studio")
  set(CMAKE_POLICY_DEFAULT_CMP0141 NEW CACHE STRING "" FORCE)
  set(CMAKE_MSVC_DEBUG_INFORMATION_FORMAT Embedded CACHE STRING "" FORCE)

  # cc-rs adds -Brepro to the flags inherited by cmake-rs. Some MSVC versions
  # fail while rewriting COFF timestamps under parallel builds (C1056), so keep
  # reproducibility at the Rust artifact layer and omit it for native objects.
  foreach(flag CMAKE_C_FLAGS CMAKE_C_FLAGS_DEBUG CMAKE_CXX_FLAGS CMAKE_CXX_FLAGS_DEBUG)
    string(REPLACE "-Brepro" "" cleaned_flags "${${flag}}")
    set(${flag} "${cleaned_flags}" CACHE STRING "" FORCE)
  endforeach()
endif()
