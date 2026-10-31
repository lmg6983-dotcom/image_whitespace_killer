# trim-image

특허 도면 이미지의 흰 여백을 잘라내고 `도 1`, `도 2`, … 로 번호를 매기는 CLI 도구입니다.
기존 `trim_image.bat`(ImageMagick `-fuzz 5% -trim` + 이름 변경 + 원본 삭제)를 Rust로 옮기면서
**AI 에이전트가 사람 개입 없이 안전하게 호출**할 수 있도록 다시 설계했습니다.

| | `trim_image.bat` | `trim-image` |
|---|---|---|
| 외부 의존성 | ImageMagick(`magick`) 필요 | 단일 실행 파일, 의존성 없음 |
| 대상 폴더 | BAT 파일이 있는 폴더 고정 | 인자로 파일/폴더 지정 (기본: 현재 폴더) |
| 실행 종료 | `pause`로 키 입력 대기 | 대기 없음 |
| 결과 확인 | 콘솔 텍스트 | `--json` 구조화 리포트 + 종료 코드 |
| 미리보기 | 없음 | `--dry-run` |
| 실패 시 | 실패 파일만 건너뛰고 나머지는 이름 변경 → 번호 밀림 | 하나라도 실패하면 **아무것도 바꾸지 않음** (`--allow-partial`로 변경 가능) |
| 기존 파일 덮어쓰기 | `move /y`로 무조건 덮어씀 | 이번 작업과 무관한 파일은 덮어쓰지 않음 (`--overwrite`로 허용) |
| 정렬 순서 | `dir` 사전순 (`fig10`이 `fig2`보다 앞) | 자연 정렬 (`fig2` → `fig10`) |
| 여백 없는 이미지 | 재압축 (JPEG 화질 손실) | 원본 바이트 그대로 복사 |
| 빈(전부 흰색) 이미지 | 1×1 등 비정상 결과 가능 | 자르지 않고 `blank`로 보고 |
| DPI | ImageMagick 기본 동작 | PNG/JPEG/TIFF/BMP의 DPI 보존 |
| EXIF 회전 | — | 회전 정보를 반영해 화면에 보이는 방향 기준으로 처리 |

## 빌드

[Rust](https://rustup.rs/) 설치 후:

```bash
cargo build --release
```

실행 파일: `target/release/trim-image` (Windows: `target\release\trim-image.exe`).
PATH에 있는 폴더로 복사해 두면 어디서든 `trim-image`로 호출할 수 있습니다.

## 사용법

```bash
# BAT와 동일: 현재 폴더의 이미지를 잘라서 '도 1.png', '도 2.jpg' … 로 저장하고 원본 삭제
trim-image

# 특정 폴더를 미리보기 (아무것도 변경하지 않음)
trim-image ./도면 --dry-run

# 원본은 그대로 두고 결과만 다른 폴더에 저장
trim-image ./도면 -o ./완성

# 파일 이름은 그대로 두고 여백만 제거 (제자리 덮어쓰기)
trim-image ./도면 --keep-names

# 'FIG01.png', 'FIG02.png' … 로 저장, 여백 10px 남기기
trim-image a.png b.tif --name 'FIG{num:02}' --margin 10
```

### 옵션

| 옵션 | 기본값 | 설명 |
|---|---|---|
| `PATHS...` | `.` | 처리할 이미지 파일 또는 폴더 (폴더는 하위 폴더 미포함) |
| `-f, --fuzz <PERCENT>` | `5` | 배경색 허용 오차 (ImageMagick `-fuzz`와 동일한 의미) |
| `-b, --bg <COLOR>` | `white` | 잘라낼 배경색: `white`, `black`, `#RRGGBB`, `auto`(네 모서리에서 추정) |
| `-n, --name <TEMPLATE>` | `도 {num}` | 출력 파일 이름(확장자 제외). `{num}`, `{num:03}`, `{stem}` 사용 가능 |
| `--keep-names` | | 원래 파일 이름 유지 (`--name '{stem}'`과 동일) |
| `--start <N>` | `1` | 시작 번호 |
| `-o, --out-dir <DIR>` | | 결과를 이 폴더에 저장. 이 경우 원본은 **삭제하지 않음** |
| `-k, --keep-originals` | | 원본을 삭제하지 않음 |
| `--overwrite` | | 이번 작업과 무관한 기존 파일도 덮어쓰기 허용 |
| `--allow-partial` | | 일부 파일이 실패해도 성공한 파일은 반영 |
| `-m, --margin <PX>` | `0` | 내용 주변에 남길 여백(픽셀) |
| `-q, --quality <Q>` | `95` | 다시 인코딩하는 JPEG의 품질 (1–100) |
| `--dry-run` | | 분석만 하고 파일을 쓰거나 지우지 않음 |
| `--json` | | 표준 출력으로 JSON 리포트 출력 |
| `-Q, --quiet` | | 파일별 출력 생략 (요약과 오류는 표시) |

지원 형식: `png jpg jpeg tif tiff bmp gif webp` (대소문자 무관). 출력 형식은 원본 확장자를 따릅니다.

### 종료 코드

| 코드 | 의미 |
|---|---|
| `0` | 모든 파일 성공 (`trimmed` / `unchanged` / `blank`), 변경 사항 반영됨 |
| `1` | 하나 이상 실패. `--allow-partial`이 없으면 **아무것도 변경되지 않음** |
| `2` | 잘못된 인자 또는 치명적 오류 (아무것도 변경되지 않음) |

## 안전 장치

1. 모든 결과를 먼저 출력 폴더 안의 임시 폴더(`.trim-image-tmp-<pid>`)에 완전히 저장합니다.
2. 하나라도 실패하면(기본값) 임시 파일을 지우고 종료합니다. 원본은 그대로입니다.
3. 결과 파일 이름이 이번 작업과 무관한 기존 파일과 겹치면 해당 파일을 실패로 처리합니다 (`--overwrite`로 허용).
4. 모두 성공했을 때만 원본을 삭제하고 결과를 최종 이름으로 옮깁니다.
5. 같은 폴더에서 다시 실행해도 안전합니다. 이미 잘린 `도 1..n`은 `unchanged`로 처리되고 원본 바이트 그대로 유지되며 번호 순서도 바뀌지 않습니다.

## 알려진 제한

- GIF는 첫 프레임만 저장됩니다 (경고 표시).
- 1비트(흑백) TIFF/PNG는 8비트 그레이스케일로 저장됩니다. 화질 차이는 없지만 파일이 조금 커질 수 있습니다.
- JPEG를 잘라내면 다시 인코딩합니다 (`--quality`, 기본 95). ICC 프로필과 EXIF는 유지되지 않습니다.
- GIF/WebP에는 DPI 정보가 없습니다.

## 에이전트 연동

[`AGENTS.md`](AGENTS.md)에 호출 방법과 JSON 형식을 정리했습니다.

## 개발

```bash
cargo test
cargo clippy --release
```

원래 BAT 파일은 참고용으로 [`legacy/trim_image.bat`](legacy/trim_image.bat)에 있습니다.
