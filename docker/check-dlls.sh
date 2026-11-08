#!/bin/sh
# Fails if a Windows binary needs a DLL that is not part of Windows (or of the NVIDIA
# driver and the CUDA DLLs that the CUDA build ships next to it).
# Usage: check-dlls.sh <exe> [objdump command]
set -eu
dlls=$("${2:-objdump}" -p "$1" | sed -n 's/.*DLL Name: *//p')
echo "linked DLLs:"; echo "$dlls"
system='^(kernel32|msvcrt|combase|bcryptprimitives|user32|gdi32|advapi32|shell32|shlwapi|ole32|oleaut32|ws2_32|secur32|bcrypt|crypt32|ncrypt|ntdll|userenv|vfw32|strmiids|mfplat|mfuuid|psapi|dbghelp|iphlpapi|winmm|version|d3d11|dxgi|api-ms-win-.*|nvcuda|cudart64_.*|cublas64_.*|cublaslt64_.*)\.dll$'
bad=$(echo "$dlls" | tr 'A-Z' 'a-z' | grep -Ev "$system" || true)
if [ -n "$bad" ]; then
    echo "error: verbacue.exe needs DLLs that are not part of Windows: $bad" >&2
    exit 1
fi
