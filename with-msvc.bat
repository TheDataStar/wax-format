@echo off
REM Local helper for Windows machines where MSVC isn't on PATH.
REM Adjust the vcvars path to your Build Tools install, then: with-msvc.bat cargo test
for /f "usebackq tokens=*" %%i in (`"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set VSPATH=%%i
call "%VSPATH%\VC\Auxiliary\Build\vcvars64.bat" >nul 2>&1
set "PATH=%PATH%;%USERPROFILE%\.cargo\bin"
%*
