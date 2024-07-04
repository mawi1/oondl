use anyhow::{anyhow, ensure, Context, Ok};
use const_format::concatcp;
use html_escape::decode_html_entities;
use lazy_static::lazy_static;
use regex::Regex;
use url::Url;

const BASE_RE: &'static str = r"https?://[-a-zA-Z0-9.]+\.apa\.at/dash/cms-(austria|worldwide|worldwide_episodes)(/[-a-zA-Z0-9_]+)*";

pub(super) fn extract_title(html: &str) -> anyhow::Result<String> {
    lazy_static! {
        static ref RE: Regex =
            Regex::new(r#"<meta\s*property="og:title"\s*content="(.*)""#).unwrap();
    }
    Ok(decode_html_entities(
        &RE.captures(&html)
            .context("could not extract title")?
            .get(1)
            .unwrap()
            .as_str()
            .to_owned(),
    )
    .into_owned())
}

pub(super) fn extract_segment_url(html: &str, segment_id: &str) -> anyhow::Result<Url> {
    lazy_static! {
        static ref RE: Regex = Regex::new(concatcp!(
            BASE_RE,
            r"/[-a-zA-Z0-9_]+__s(?<segment_id>[0-9]+)_[-a-zA-Z0-9_]+_QXB\.mp4/manifest\.mpd"
        ))
        .unwrap();
    }
    RE.captures_iter(html)
        .find(|c| &c["segment_id"] == segment_id)
        .map(|c| Url::parse(&c[0]).unwrap())
        .ok_or_else(|| anyhow!("could not extract segment url"))
}

#[derive(Debug)]
pub(super) enum VideoInfo {
    Unsegmented(Url),
    Segmented(Vec<Url>),
}

pub(super) fn extract_video_info(html: &str) -> anyhow::Result<VideoInfo> {
    lazy_static! {
        static ref UNSEGMENTED_RE: Regex =
            Regex::new(concatcp!(BASE_RE, r"/[0-9]+_[0-9]+_QXB\.mp4/manifest\.mpd")).unwrap();
    }
    if let Some(url) = UNSEGMENTED_RE
        .find(html)
        .map(|m| Url::parse(m.as_str()).unwrap())
    {
        Ok(VideoInfo::Unsegmented(url))
    } else {
        lazy_static! {
            static ref ALL_MPD_RE: Regex = Regex::new(concatcp!(
                BASE_RE,
                r"/[-a-zA-Z0-9_]+_QXB\.mp4/manifest\.mpd"
            ))
            .unwrap();
        }
        let urls = ALL_MPD_RE
            .find_iter(&html)
            .map(|m| Url::parse(m.as_str()).unwrap())
            .collect::<Vec<_>>();
        ensure!(!urls.is_empty(), "could not extract mpd-urls");

        if urls.len() == 1 {
            Ok(VideoInfo::Unsegmented(urls[0].clone()))
        } else {
            Ok(VideoInfo::Segmented(urls))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;
    use std::path::PathBuf;

    use insta::assert_debug_snapshot;

    use super::*;

    fn get_test_html(file_name: &str) -> String {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "test_files", file_name]
            .iter()
            .collect();
        read_to_string(path).unwrap()
    }

    #[test]
    fn test_extract_title() {
        let title: String = extract_title(&get_test_html("title.html")).unwrap();
        assert_eq!(title, "ZIB 1 vom 08.05.2024");
    }

    #[test]
    fn test_extract_title_escaped() {
        let title: String = extract_title(&get_test_html("title_escaped.html")).unwrap();
        assert_eq!(title, "ORF-Hilfsaktion \"Österreich hilft Österreich\" - Wien heute vom 13.06.2024");
    }

    #[test]
    fn test_extract_segmented() {
        let u = extract_video_info(&get_test_html("segmented.html"));
        assert_debug_snapshot!(u);
    }

    #[test]
    fn test_extract_unsegmented() {
        let u = extract_video_info(&get_test_html("unsegmented.html"));
        assert_debug_snapshot!(u);
    }

    #[test]
    fn test_extract_segmented_and_unsegmented() {
        let u = extract_video_info(&get_test_html("segmented_and_unsegmented.html"));
        assert_debug_snapshot!(u);
    }

    #[test]
    fn test_extract_segment_url() {
        let u = extract_segment_url(&get_test_html("segment.html"), "15658303");
        assert_debug_snapshot!(u);
    }

    #[test]
    fn test_extract_without_bumper_clip() {
        let u = extract_video_info(&get_test_html("with_bumper_clip.html"));
        assert_debug_snapshot!(u);
    }
}
