# 📝 변경 내역

이 프로젝트의 주요 변경 사항을 모두 기록합니다.
형식은 [Keep a Changelog](https://keepachangelog.com/ko/1.1.0/)를, 버전은 [유의적 버전](https://semver.org/lang/ko/)을 따릅니다.

## [Unreleased]

## [0.2.1] - 2026-11-05

### 🔧 바뀐 점

- 💄 확인 창·진행 창에 여백을 넉넉히 두어 글자가 테두리에 붙지 않게 했습니다

### 📝 문서

- README 미리보기를 실제 사용 화면으로 교체하고, 원본과 결과 비교 이미지를 추가했습니다

## [0.2.0] - 2026-11-05

### ✨ 새 기능

- **데스크톱 앱 `trim-image-gui`** — 이미지를 끌어다 놓기만 하면 여백을 잘라 주는 창 프로그램
  - 👀 자르기 전에 잘려 나갈 부분을 어둡게 미리 보여 줍니다
  - 🔢 목록 순서를 바꿔 `도 1`, `도 2` … 번호를 정할 수 있습니다
  - 🛡️ 원본을 삭제하기 전에 한 번 더 확인합니다
  - 💾 설정을 기억해 다음 실행 때 그대로 씁니다
  - ⌨️ 단축키: <kbd>Ctrl</kbd>+<kbd>O</kbd> 파일 추가, <kbd>Ctrl</kbd>+<kbd>Enter</kbd> 실행, <kbd>Alt</kbd>+<kbd>↑</kbd>/<kbd>↓</kbd> 순서 변경
- 명령줄 도구의 JSON 결과에 실패 원인 코드 `error_code` 추가

### 🔧 바뀐 점

- 명령줄 도구(`trim-image`)와 데스크톱 앱(`trim-image-gui`)을 따로 내려받을 수 있습니다
- 사용자 안내 위주로 README를 새로 썼습니다
- MIT 라이선스 적용

## [0.1.0] - 2026-10-31

### ✨ 새 기능

- 🎉 첫 버전: 명령줄 도구 `trim-image`
  - 흰 여백 자르기, `도 1` · `도 2` … 이름 붙이기, 원본 정리
  - 미리보기(`--dry-run`), JSON 결과(`--json`), 실패 시 전체 취소

[Unreleased]: https://github.com/lmg6983-dotcom/image_whitespace_killer/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/lmg6983-dotcom/image_whitespace_killer/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/lmg6983-dotcom/image_whitespace_killer/compare/6088c72...v0.2.0
[0.1.0]: https://github.com/lmg6983-dotcom/image_whitespace_killer/tree/6088c72
