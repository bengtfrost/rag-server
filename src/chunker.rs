use regex::Regex;

pub fn split_into_sentences(text: &str) -> Vec<String> {
    let re_newline = Regex::new(r"(?<!\n)\n(?!\n)").unwrap();
    let text = re_newline.replace_all(text, " ");
    let re_sentence = Regex::new(r"(?<=[.!?])\s+(?=[A-ZÅÄÖ])").unwrap();
    re_sentence.split(&text)
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() > 2)
        .collect()
}

fn count_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

pub fn chunk_text_exact(text: &str, max_tokens: usize, overlap_tokens: usize) -> anyhow::Result<Vec<String>> {
    let sentences = split_into_sentences(text);
    if sentences.is_empty() {
        return Ok(Vec::new());
    }

    let sentence_tokens: Vec<usize> = sentences.iter().map(|s| count_tokens(s)).collect();

    let mut chunks = Vec::new();
    let mut current_sents: Vec<String> = Vec::new();
    let mut current_tokens = 0;
    let mut current_sent_tokens: Vec<usize> = Vec::new();

    for (_i, (sent, &s_tok)) in sentences.iter().zip(sentence_tokens.iter()).enumerate() {
        if s_tok > max_tokens {
            if !current_sents.is_empty() {
                chunks.push(current_sents.join(" "));
                current_sents.clear();
                current_sent_tokens.clear();
                current_tokens = 0;
            }
            let truncated: String = sent.chars().take(max_tokens * 4).collect();
            chunks.push(truncated);
            continue;
        }

        if current_tokens + s_tok > max_tokens && !current_sents.is_empty() {
            chunks.push(current_sents.join(" "));

            let mut overlap_sents: Vec<String> = Vec::new();
            let mut overlap_tok = 0;
            for (s, t) in current_sents.iter().rev().zip(current_sent_tokens.iter().rev()) {
                if overlap_tok + t > overlap_tokens {
                    break;
                }
                overlap_sents.insert(0, s.clone());
                overlap_tok += t;
            }
            current_sents = overlap_sents;
            let new_tokens: Vec<usize> = current_sents.iter().map(|s| count_tokens(s)).collect();
            current_sent_tokens = new_tokens;
            current_tokens = current_sent_tokens.iter().sum();
        }

        current_sents.push(sent.clone());
        current_sent_tokens.push(s_tok);
        current_tokens += s_tok;
    }

    if !current_sents.is_empty() {
        chunks.push(current_sents.join(" "));
    }

    Ok(chunks)
}