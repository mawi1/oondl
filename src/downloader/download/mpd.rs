use anyhow::{anyhow, bail, ensure, Context};
use roxmltree::{Document, Node};
use url::Url;

use super::super::Quality;

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Literal(&'a str),
    Time,
    RepresentationID,
}

impl<'a> Token<'a> {
    fn scan(input: &'a str) -> anyhow::Result<Vec<Token<'a>>> {
        let mut tokens = vec![];
        let mut chars = input.chars().enumerate().peekable();

        while let Some((idx, c)) = chars.next() {
            if c == '$' {
                let mut var_name = String::new();
                loop {
                    if let Some((_, v)) = chars.next() {
                        if v == '$' {
                            match var_name.as_str() {
                                "Time" => tokens.push(Token::Time),
                                "RepresentationID" => tokens.push(Token::RepresentationID),
                                _ => bail!("invalid template variable: {}", var_name),
                            }
                            break;
                        } else {
                            var_name.push(v);
                        }
                    } else {
                        bail!("unterminated variable");
                    }
                }
            } else {
                loop {
                    let maybe_pc = chars.peek();
                    if let Some((pi, pc)) = maybe_pc {
                        if *pc == '$' {
                            tokens.push(Token::Literal(&input[idx..*pi]));
                            break;
                        } else {
                            chars.next();
                        }
                    } else {
                        tokens.push(Token::Literal(&input[idx..]));
                        break;
                    }
                }
            }
        }

        Ok(tokens)
    }
}

struct SegmentTemplate<'a> {
    base_url: &'a Url,
    tokens: Vec<Token<'a>>,
}

impl<'a> SegmentTemplate<'a> {
    fn new(base_url: &'a Url, template: &'a str) -> anyhow::Result<Self> {
        Ok(Self {
            base_url,
            tokens: Token::scan(&template)?,
        })
    }

    fn render(&self, representation_id: &str, maybe_time: Option<u64>) -> Url {
        let time = maybe_time.map_or("".to_owned(), |t| t.to_string());
        let path = self
            .tokens
            .iter()
            .map(|t| match *t {
                Token::Literal(s) => s,
                Token::Time => &time,
                Token::RepresentationID => representation_id,
            })
            .collect::<String>();

        self.base_url.join(&path).unwrap()
    }
}

struct Segment {
    maybe_time: Option<u64>,
    duration: u64,
    maybe_repeat: Option<u64>, //todo coorect datatypes
}

#[derive(Debug)]
pub struct MediaUrls {
    pub video: Vec<Url>,
    pub audio: Vec<Url>,
}

fn node_not_found(name: &'static str) -> anyhow::Error {
    anyhow!("node not found: {}", name)
}

fn urls_from_adaptation_set(
    base_url: &Url,
    as_node: Node,
    maybe_quality: Option<Quality>,
) -> anyhow::Result<Vec<Url>> {
    let representation_id = if let Some(quality) = maybe_quality {
        let representations = as_node
            .children()
            .filter(|c| c.has_tag_name("Representation"))
            .map(|n| {
                let id = n
                    .attribute("id")
                    .ok_or_else(|| node_not_found("Representation[@id]"))?;
                let bandwith = n
                    .attribute("bandwidth")
                    .ok_or_else(|| node_not_found(""))?
                    .parse::<u32>()
                    .context("could not parse bandwidth")?;

                Ok((id, bandwith))
            })
            .collect::<anyhow::Result<Vec<(&str, u32)>>>()?;
        ensure!(!representations.is_empty(), "no representation nodes found");

        match quality {
            Quality::Low => representations
                .iter()
                .min_by_key(|(_, bandwidth)| bandwidth)
                .map(|(id, _)| *id)
                .unwrap(),
            Quality::Medium => {
                let avg_bandwith = representations
                    .iter()
                    .map(|(_, bandwdth)| *bandwdth)
                    .sum::<u32>()
                    / representations.len() as u32;
                representations
                    .iter()
                    .min_by_key(|(_, bandwidth)| avg_bandwith.abs_diff(*bandwidth))
                    .map(|(id, _)| *id)
                    .unwrap()
            }
            Quality::High => representations
                .iter()
                .max_by_key(|(_, bandwidth)| bandwidth)
                .map(|(id, _)| *id)
                .unwrap(),
        }
    } else {
        as_node
            .children()
            .find(|c| c.has_tag_name("Representation"))
            .ok_or_else(|| node_not_found("Representation"))?
            .attribute("id")
            .ok_or_else(|| node_not_found("Representation[@id]"))?
    };

    let segment_template = as_node
        .children()
        .find(|c| c.has_tag_name("SegmentTemplate"))
        .ok_or_else(|| node_not_found("SegmentTemplate"))?;
    let init_template = segment_template
        .attribute("initialization")
        .ok_or_else(|| node_not_found("SegmentTemplate[@intialization]"))?;
    let template = segment_template
        .attribute("media")
        .ok_or_else(|| node_not_found("SegmentTemplate[@media]"))?;

    let segments = segment_template
        .children()
        .find(|c| c.has_tag_name("SegmentTimeline"))
        .ok_or_else(|| node_not_found("SegmentTimeline"))?
        .children()
        .filter(|c| c.has_tag_name("S"))
        .map(|c| {
            let maybe_time = c
                .attribute("time")
                .map(|t| t.parse::<u64>().context("could not parse time"))
                .transpose()?;
            let duration = c
                .attribute("d")
                .ok_or_else(|| node_not_found("S[@d]"))?
                .parse::<u64>()
                .context("could not parse duration")?;
            let maybe_repeat = c
                .attribute("r")
                .map(|r| r.parse::<u64>().context("could not parse repeat"))
                .transpose()?;

            Ok(Segment {
                maybe_time,
                duration,
                maybe_repeat,
            })
        })
        .collect::<anyhow::Result<Vec<Segment>>>()?;
    ensure!(segments.len() > 0, "no segments found");

    let init_seg_template = SegmentTemplate::new(base_url, init_template)?;
    let mut urls = vec![init_seg_template.render(representation_id, None)];

    let seg_template = SegmentTemplate::new(base_url, template)?;
    let mut last_end_time = 0;
    for s in segments {
        let mut start_time = if let Some(t) = s.maybe_time {
            t
        } else {
            last_end_time
        };
        for _ in 0..=s.maybe_repeat.unwrap_or(0) {
            let u = seg_template.render(representation_id, Some(start_time));
            urls.push(u);

            let end_time = start_time + s.duration;
            start_time = end_time;
            last_end_time = end_time;
        }
    }

    Ok(urls)
}

pub(super) fn get_urls(base_url: &Url, xml: &str, quality: Quality) -> anyhow::Result<MediaUrls> {
    let doc = Document::parse(xml)?;
    let period = doc
        .root_element()
        .children()
        .find(|c| c.has_tag_name("Period"))
        .ok_or_else(|| node_not_found("Period"))?;
    let video_as = period
        .children()
        .find(|c| c.attribute("mimeType") == Some("video/mp4"))
        .ok_or_else(|| node_not_found("AdaptationSet[@mimeType=video/mp4]"))?;
    let audio_as = period
        .children()
        .find(|c| c.attribute("mimeType") == Some("audio/mp4"))
        .ok_or_else(|| node_not_found("AdaptationSet[@mimeType=audio/mp4]"))?;

    Ok(MediaUrls {
        video: urls_from_adaptation_set(base_url, video_as, Some(quality))?,
        audio: urls_from_adaptation_set(base_url, audio_as, None)?,
    })
}

#[cfg(test)]
mod tests {
    use std::fs::read_to_string;
    use std::path::PathBuf;

    use insta::assert_debug_snapshot;

    use super::*;

    #[test]
    fn test_token() {
        let t1 = Token::scan("abc_$Time$123").unwrap();
        let e1 = vec![Token::Literal("abc_"), Token::Time, Token::Literal("123")];
        assert_eq!(t1, e1);

        let t2 = Token::scan("abc_$Time$").unwrap();
        let e2 = vec![Token::Literal("abc_"), Token::Time];
        assert_eq!(t2, e2);

        let t3 = Token::scan("$Time$__eee333").unwrap();
        let e3 = vec![Token::Time, Token::Literal("__eee333")];
        assert_eq!(t3, e3);
    }

    #[test]
    fn test_invalid_variable() {
        let t = Token::scan("abc_$Foo$");
        assert_eq!(t.unwrap_err().to_string(), "invalid template variable: Foo");
    }

    #[test]
    fn test_unterminated_variable() {
        let t = Token::scan("abc_$Foo");
        assert_eq!(t.unwrap_err().to_string(), "unterminated variable");
    }

    #[test]
    fn test_seg_templ() {
        let base_url = Url::parse("http://example.com/123/abc/321/manifest.mpd").unwrap();
        let template = "seg_$RepresentationID$_foo$Time$_mpd.m4s";
        let representation_id = "v123xyz";

        let s = SegmentTemplate::new(&base_url, &template).unwrap();
        assert_eq!(
            s.render(representation_id, Some(500)).to_string(),
            "http://example.com/123/abc/321/seg_v123xyz_foo500_mpd.m4s"
        );
        assert_eq!(
            s.render(representation_id, Some(800)).to_string(),
            "http://example.com/123/abc/321/seg_v123xyz_foo800_mpd.m4s"
        );
    }

    fn get_test_mpd() -> (Url, String) {
        let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "test_files", "manifest.mpd"]
            .iter()
            .collect();
        (
            Url::parse("http://example.com/123/abc/321/manifest.mpd").unwrap(),
            read_to_string(path).unwrap(),
        )
    }

    #[test]
    fn test_mpd_low() {
        let (base_url, xml) = get_test_mpd();
        let r = get_urls(&base_url, &xml, Quality::Low);
        assert_debug_snapshot!(r);
    }

    #[test]
    fn test_mpd_medium() {
        let (base_url, xml) = get_test_mpd();
        let r = get_urls(&base_url, &xml, Quality::Medium);
        assert_debug_snapshot!(r);
    }

    #[test]
    fn test_mpd_high() {
        let (base_url, xml) = get_test_mpd();
        let r = get_urls(&base_url, &xml, Quality::High);
        assert_debug_snapshot!(r);
    }
}
