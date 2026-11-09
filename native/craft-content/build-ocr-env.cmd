@echo off
setlocal
rem Arguments are verified absolute vcvarsall, vcpkg, task scratch, toolset.
rem The host PATH is near cmd.exe's 8191-character expansion limit.
set "PATH=%MAILCRAFT_OCR_BASE_PATH%"
call "%~1" amd64 -vcvars_ver=%~4
if errorlevel 1 exit /b %errorlevel%
"%~2" install --triplet mailcraft-x64-explicit --host-triplet mailcraft-x64-explicit ^
 "--x-manifest-root=%~3\ocr" "--overlay-ports=%~3\ocr\ports" "--overlay-triplets=%~3\ocr\triplets" ^
 "--x-install-root=%~3\ocr\installed" "--x-buildtrees-root=%~3\ocr\buildtrees" ^
 "--x-packages-root=%~3\ocr\packages" "--downloads-root=%~3\ocr\downloads"
exit /b %errorlevel%
