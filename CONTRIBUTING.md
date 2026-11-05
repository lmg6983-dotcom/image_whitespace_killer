# 🤝 기여 안내

## 커밋 메시지

[gitmoji](https://gitmoji.dev/) + [Conventional Commits](https://www.conventionalcommits.org/ko/v1.0.0/) 형식을 씁니다.

```text
<이모지> <종류>(<범위>): <무엇을 했는지 한 줄 요약>

<왜 바꿨는지, 무엇이 달라지는지 (선택)>
```

| 이모지 | 종류 | 언제 |
|:---:|---|---|
| ✨ | `feat` | 새 기능 |
| 🐛 | `fix` | 버그 수정 |
| 📝 | `docs` | 문서 (README, CHANGELOG 등) |
| 💄 | `style` | 화면 디자인 수정 |
| ♻️ | `refactor` | 동작은 같고 구조만 개선 |
| ⚡ | `perf` | 속도 개선 |
| ✅ | `test` | 테스트 |
| 👷 | `ci` | 빌드·배포 자동화 |
| 🔖 | `release` | 버전 올리기 |

범위는 `gui`, `cli`, `core` 중 하나를 씁니다. 예: `🐛 fix(gui): 미리보기가 갱신되지 않던 문제 수정`

## 배포

1. `Cargo.toml`의 `version`을 올립니다.
2. `CHANGELOG.md`의 `[Unreleased]` 내용을 새 버전 항목으로 옮깁니다.
3. Actions → **build** → **Run workflow**에서 `release`를 체크하고 실행합니다.

릴리스 페이지와 다운로드 링크, 변경 내역은 자동으로 만들어집니다.
