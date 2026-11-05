//! User-facing Korean wording. The engine reports in English (for scripts and
//! agents); everything shown in the app is translated here.

use trim_core::{ErrorCode, FileReport, Status};

/// Short status label for a file row.
pub fn status_label(status: Status, done: bool) -> &'static str {
    match (status, done) {
        (Status::Trimmed, false) => "자를 예정",
        (Status::Trimmed, true) => "완료",
        (Status::Unchanged, _) => "여백 없음",
        (Status::Blank, _) => "빈 이미지",
        (Status::Failed, _) => "오류",
    }
}

/// Explanation of a failed file, with the next step.
pub fn file_error(f: &FileReport) -> String {
    let file = file_name(&f.output);
    match f.error_code {
        Some(ErrorCode::Read) => {
            "파일을 읽을 수 없습니다. 다른 프로그램에서 열려 있지 않은지 확인하세요.".into()
        }
        Some(ErrorCode::Decode) => {
            "이미지를 열 수 없습니다. 파일이 손상되었거나 지원하지 않는 형식입니다.".into()
        }
        Some(ErrorCode::Encode) => "결과 이미지를 만들지 못했습니다.".into(),
        Some(ErrorCode::Write) => {
            "결과를 저장하지 못했습니다. 폴더 권한과 남은 디스크 공간을 확인하세요.".into()
        }
        Some(ErrorCode::Exists) => format!(
            "‘{file}’ 파일이 이미 있습니다. 다른 이름 형식이나 저장 폴더를 고르거나, 고급 설정에서 덮어쓰기를 켜세요."
        ),
        None => f.error.clone().unwrap_or_else(|| "알 수 없는 오류".into()),
    }
}

/// Translate an engine warning.
pub fn warning(w: &str) -> String {
    if w.contains("entirely background") {
        "이미지 전체가 배경색이라 자르지 않았습니다. 내용을 직접 확인하세요.".into()
    } else if w.contains("only the first frame") {
        "움직이는 GIF는 첫 장면만 저장됩니다.".into()
    } else if w.contains("no DPI") {
        "원본에 해상도(DPI) 정보가 없습니다.".into()
    } else if w.contains("could not delete original") {
        "원본 파일을 삭제하지 못했습니다.".into()
    } else {
        w.to_string()
    }
}

/// Translate a batch-level (fatal) error.
pub fn fatal(e: &str) -> String {
    if e.contains("several inputs would be written") {
        "여러 파일이 같은 이름으로 저장됩니다. 이름 형식에 {번호}를 넣으세요.".into()
    } else if e.contains("is the input file itself") {
        "원본을 남기면서 같은 이름으로 저장할 수는 없습니다. 이름 형식이나 저장 폴더를 바꾸세요."
            .into()
    } else if e.contains("template") || e.contains("placeholder") || e.contains('{') {
        format!("이름 형식이 올바르지 않습니다. ({e})")
    } else if e.contains("cannot access") {
        "목록의 파일 중 일부를 찾을 수 없습니다. 이동되거나 삭제되었는지 확인하세요.".into()
    } else if e.contains("output directory") || e.contains("not a directory") {
        "저장 폴더를 만들 수 없습니다. 다른 폴더를 고르세요.".into()
    } else {
        e.to_string()
    }
}

pub fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}
