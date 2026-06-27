use std::path::Path;

pub fn extract_text_from_file(file_path: &str, _encoding: Option<&str>) -> anyhow::Result<String> {
    let path = Path::new(file_path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    if ext == "pdf" {
        extract_pdf(file_path)
    } else {
        let content = std::fs::read_to_string(file_path)?;
        Ok(content)
    }
}

fn extract_pdf(file_path: &str) -> anyhow::Result<String> {
    use pdf_extract::extract_text;
    let text = extract_text(file_path)?;
    Ok(text)
}