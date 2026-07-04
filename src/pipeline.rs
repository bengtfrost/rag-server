use reqwest::Client;
use tokio::sync::mpsc;
use tracing::debug;

use crate::config::Config;
use crate::embedder::get_embeddings;
use crate::chunker::BgeChunker;

pub struct IngestionPipeline {
    chunker: BgeChunker,
    cfg: Config,
    client: Client,
}

impl IngestionPipeline {
    pub fn new(cfg: &Config, client: Client) -> anyhow::Result<Self> {
        let chunker = BgeChunker::new(&cfg.tokenizer_path)?;
        Ok(Self {
            chunker,
            cfg: cfg.clone(),
            client,
        })
    }

    pub async fn process(
        &self,
        text: &str,
        _collection: &str,
        doc_id: &str,
        _force: bool,
    ) -> anyhow::Result<Vec<String>> {
        let sentences = self.chunker.split_sentences(text);
        let total_sentences = sentences.len();

        if total_sentences == 0 {
            return Ok(Vec::new());
        }

        debug!(
            "Pipeline: Processing {} sentences for {}",
            total_sentences, doc_id
        );

        let (tx, mut rx) = mpsc::channel::<Vec<String>>(self.cfg.embed_batch_size);
        let cfg_clone = self.cfg.clone();
        let client_clone = self.client.clone();
        let chunker = self.chunker.clone();  // clone to move into task
        let max_tokens = self.cfg.chunk_size;
        let overlap_tokens = self.cfg.chunk_overlap;

        // Producer: Generate chunks from sentences
        let producer = tokio::spawn(async move {
            let mut buffer = Vec::new();
            let mut current_chunk: Vec<String> = Vec::new();
            let mut current_tokens = 0;

            for sent in sentences {
                let tokens = chunker.count_tokens(&sent);
                if current_tokens + tokens > max_tokens && !current_chunk.is_empty() {
                    let chunk_text = current_chunk.join(" ");
                    buffer.push(chunk_text);

                    if buffer.len() >= cfg_clone.embed_batch_size {
                        if tx.send(buffer).await.is_err() {
                            return Ok::<_, anyhow::Error>(());
                        }
                        buffer = Vec::new();
                    }

                    // Handle overlap
                    let mut overlap_vec: Vec<String> = Vec::new();
                    let mut overlap_count = 0;
                    for s in current_chunk.iter().rev() {
                        let sn = chunker.count_tokens(s);
                        if overlap_count + sn <= overlap_tokens {
                            overlap_vec.insert(0, s.clone());
                            overlap_count += sn;
                        } else {
                            break;
                        }
                    }
                    current_chunk = overlap_vec;
                    current_tokens = overlap_count;
                }
                current_chunk.push(sent);
                current_tokens += tokens;
            }

            if !current_chunk.is_empty() {
                let chunk_text = current_chunk.join(" ");
                buffer.push(chunk_text);
            }

            if !buffer.is_empty() {
                let _ = tx.send(buffer).await;
            }

            Ok::<_, anyhow::Error>(())
        });

        // Consumer: Generate embeddings for chunks
        let consumer = tokio::spawn(async move {
            let mut all_chunks = Vec::new();
            let mut all_embeddings = Vec::new();

            while let Some(chunks) = rx.recv().await {
                let embs = get_embeddings(&client_clone, &cfg_clone, &chunks, "pipeline").await?;
                all_chunks.extend(chunks);
                all_embeddings.extend(embs);
            }

            Ok::<_, anyhow::Error>((all_chunks, all_embeddings))
        });

        // Wait for producer to finish
        let _ = producer.await??;

        // Collect results from consumer
        let (chunks, _embeddings) = consumer.await??;

        Ok(chunks)
    }
}