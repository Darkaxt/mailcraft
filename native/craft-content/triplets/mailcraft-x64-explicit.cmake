# The caller initializes the exact component-qualified MSVC environment.
# Track these inputs in the ABI instead of re-enumerating Visual Studio.
set(VCPKG_TARGET_ARCHITECTURE x64)
set(VCPKG_CRT_LINKAGE static)
set(VCPKG_LIBRARY_LINKAGE static)
set(VCPKG_BUILD_TYPE release)
set(VCPKG_LOAD_VCVARS_ENV OFF)
set(VCPKG_ENV_PASSTHROUGH
    PATH INCLUDE LIB LIBPATH
    WindowsSdkDir WindowsSDKVersion
    VCToolsInstallDir VCToolsVersion VSCMD_ARG_TGT_ARCH
    _CL_ CMAKE_BUILD_PARALLEL_LEVEL)
