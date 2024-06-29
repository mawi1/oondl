use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use futures_util::StreamExt;
use reqwest::{Client, ClientBuilder};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use url::Url;

use super::Error;

pub struct Response {
    pub body: String,
    pub final_url: Url,
}

pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new() -> Self {
        let client = ClientBuilder::new()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("could not build reqwest client");
        Self { client }
    }

    async fn try_get(&self, url: Url) -> Result<reqwest::Response, Error> {
        let resp_result = self.client.get(url).send().await;
        match resp_result {
            Ok(resp) => match resp.error_for_status() {
                Ok(ok_status_resp) => Ok(ok_status_resp),
                Err(status_error) => Err(Error::UnexpectedError(anyhow!(status_error))),
            },
            Err(e) => Err(Error::NetworkError(e)),
        }
    }

    pub async fn get(&self, url: Url) -> Result<Response, Error> {
        let res = self.try_get(url).await?;
        let final_url = res.url().clone();

        Ok(Response {
            body: res.text().await?,
            final_url,
        })
    }

    pub async fn download_to_file(
        &self,
        dest: &Path,
        chunk_urls: Vec<Url>,
        on_chunk_downloaded: Arc<Mutex<impl FnMut()>>,
    ) -> Result<(), Error> {
        let mut file = File::create(dest).await?;

        for url in chunk_urls {
            let mut stream = self.try_get(url).await?.bytes_stream();
            while let Some(item) = stream.next().await {
                let bytes = item?;
                file.write_all(&bytes).await?;
            }
            on_chunk_downloaded.lock().unwrap()();
        }
        Ok(())
    }
}
