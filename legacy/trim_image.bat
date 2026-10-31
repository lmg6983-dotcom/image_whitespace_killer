@echo off
chcp 65001 >nul
setlocal EnableExtensions EnableDelayedExpansion

rem ===== 항상 BAT가 있는 폴더에서 작업 =====
pushd "%~dp0" 2>nul
if errorlevel 1 (
  echo 오류: 배치 파일 위치로 이동할 수 없습니다: "%~dp0"
  echo 네트워크/권한 문제를 확인하세요.
  echo.
  pause
  exit /b 1
)

rem ===== 사용자 설정 =====
set "FUZZ=5"
set "BGCOLOR=white"
rem ========================

where magick >nul 2>&1
if errorlevel 1 (
  echo ImageMagick 'magick' 실행 파일을 찾을 수 없습니다. PATH 설정 또는 설치를 확인하세요.
  echo.
  pause
  popd
  exit /b 1
)

set "COUNT=0"
set "FOUND_ANY="

rem 이름 충돌 방지를 위한 임시 폴더 생성
set "TEMPDIR=.temp_crop"
mkdir "%TEMPDIR%" 2>nul

rem 대소문자 확장자 모두 지원 (Windows dir은 대소문자 구분하지 않으므로 간소화)
set "PATTERNS=*.png *.jpg *.jpeg *.tif *.tiff *.bmp *.gif *.webp"

for /f "delims=" %%F in ('dir /b /a-d %PATTERNS% 2^>nul') do (
  set "FOUND_ANY=1"
  set /a COUNT+=1
  
  rem 새 파일명 설정 (예: 도 1.jpg, 도 2.png)
  set "NEW_NAME=도 !COUNT!%%~xF"
  echo [!COUNT!] 처리 중: "%%F" -^> "!NEW_NAME!"
  
  magick "%%F" -fuzz !FUZZ!%% -bordercolor !BGCOLOR! -trim +repage "%TEMPDIR%\!NEW_NAME!"
  
  if errorlevel 1 (
    echo    -^> 오류: "%%F" 처리 실패
  ) else (
    rem 성공적으로 자르기 완료 시 원본 파일 삭제
    del "%%F"
  )
)

if not defined FOUND_ANY (
  echo 처리할 이미지 파일이 없습니다.
  echo 현재 작업 폴더: %CD%
  rmdir "%TEMPDIR%" 2>nul
) else (
  rem 임시 폴더의 결과물들을 현재 폴더로 모두 이동 후 임시 폴더 삭제
  move /y "%TEMPDIR%\*" . >nul
  rmdir "%TEMPDIR%" 2>nul
  echo.
  echo 완료: %COUNT%개 파일 처리됨. 원본이 삭제되고 '도 1~n' 형식으로 저장되었습니다.
)

echo.
pause
popd