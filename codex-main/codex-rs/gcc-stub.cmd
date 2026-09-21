@echo off
REM gcc-stub.cmd - Fake gcc that accepts all arguments and creates empty output files
REM For sandboxed cargo check (type-checking only, no actual C compilation needed)

REM Handle version queries
echo %1 | findstr /r "^--version ^-v ^-dumpversion ^-dumpfullversion" >nul 2>&1
if not errorlevel 1 (
    echo gcc (stub) 16.2.0
    exit /b 0
)

REM Handle -print-search-dirs
echo %1 | findstr "^-print" >nul 2>&1
if not errorlevel 1 (
    echo install: /usr/lib/gcc/x86_64-w64-mingw32/16.2.0/
    exit /b 0
)

REM Handle -print-file-name
echo %1 | findstr "^-print-file-name" >nul 2>&1
if not errorlevel 1 (
    REM Just echo back the library name as if it's found
    echo %2
    exit /b 0
)

REM Find the -o argument and create the output file
set "outfile="
set "compiling="
:parse
if "%~1"=="" goto done
if "%~1"=="-o" (
    set "outfile=%~2"
    shift
    shift
    goto parse
)
if "%~1"=="-c" (
    set "compiling=1"
    shift
    goto parse
)
if "%~1"=="-E" (
    REM Preprocessor mode - output empty
    shift
    goto parse
)
if "%~1"=="-S" (
    REM Assembly mode - output empty
    shift
    goto parse
)
shift
goto parse

:done
if defined outfile (
    type nul > "%outfile%" 2>nul
)
exit /b 0
