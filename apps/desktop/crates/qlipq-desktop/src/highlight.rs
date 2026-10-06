use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use iced::futures::future::{select, Either};
use image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ExtendedColorType, ImageEncoder,
};
use qlipq_core::highlight::{
    analysis_windows, detection_schema, AnalysisWindow, HighlightDetection, HighlightSuggestion,
};
use qlipq_core::media::MediaInfo;
use serde::Deserialize;
use serde_json::{json, Value};

const OLLAMA_URL: &str = "http://127.0.0.1:11434";

#[derive(Debug)]
pub struct HighlightJob {
    pub cancel: Arc<AtomicBool>,
    pub section: Arc<AtomicUsize>,
    pub total: usize,
}

impl HighlightJob {
    pub fn new(duration: f64) -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            section: Arc::new(AtomicUsize::new(0)),
            total: analysis_windows(duration).len(),
        }
    }

    pub fn matches(&self, token: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.cancel, token) && !self.cancel.load(Ordering::Relaxed)
    }
}

impl Drop for HighlightJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct HighlightRequest {
    pub path: String,
    pub media: MediaInfo,
    pub is_hdr: bool,
    pub gamma: f64,
    pub model: String,
    pub query: String,
}

struct Sample {
    time: f64,
    image: String,
}

pub async fn detect(
    request: HighlightRequest,
    cancel: Arc<AtomicBool>,
    section: Arc<AtomicUsize>,
) -> Result<Option<HighlightSuggestion>, String> {
    #[cfg(test)]
    let test_endpoint = TEST_ENDPOINT.lock().unwrap().clone();
    #[cfg(test)]
    if let Some(base) = test_endpoint {
        return detect_at(request, cancel, section, &base).await;
    }
    detect_at(request, cancel, section, OLLAMA_URL).await
}

#[cfg(test)]
pub static TEST_ENDPOINT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

async fn detect_at(
    request: HighlightRequest,
    cancel: Arc<AtomicBool>,
    section: Arc<AtomicUsize>,
    base: &str,
) -> Result<Option<HighlightSuggestion>, String> {
    let wait_cancelled = async {
        while !cancel.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    match select(
        Box::pin(detect_inner(request, cancel.clone(), section, base)),
        Box::pin(wait_cancelled),
    )
    .await
    {
        Either::Left((result, _)) => result,
        Either::Right(_) => Err("Highlight detection cancelled.".into()),
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())
}

async fn detect_inner(
    request: HighlightRequest,
    cancel: Arc<AtomicBool>,
    section: Arc<AtomicUsize>,
    base: &str,
) -> Result<Option<HighlightSuggestion>, String> {
    let windows = analysis_windows(request.media.duration_sec);
    if windows.is_empty() {
        return Err("This clip has no usable duration for highlight detection.".into());
    }
    let model = request.model.trim();
    if model.is_empty() {
        return Err("Choose a local vision model in Settings → Highlight suggestions.".into());
    }
    let client = client()?;
    let info = post_json(&client, base, "show", json!({ "model": model })).await?;
    validate_model(&info)?;
    let mut best: Option<HighlightSuggestion> = None;
    for (index, window) in windows.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Highlight detection cancelled.".into());
        }
        section.store(index + 1, Ordering::Relaxed);
        let path = request.path.clone();
        let media = request.media.clone();
        let is_hdr = request.is_hdr;
        let gamma = request.gamma;
        let token = cancel.clone();
        let samples = tokio::task::spawn_blocking(move || {
            sample_window(&path, &media, is_hdr, gamma, window, &token)
        })
        .await
        .map_err(|e| format!("Could not sample the clip: {e}"))??;
        let detection =
            query_window(&client, base, model, &request.query, window, &samples).await?;
        if let Some(event) = detection.event {
            let suggestion = event.into_suggestion(window, request.media.duration_sec)?;
            if best
                .as_ref()
                .is_none_or(|previous| suggestion.score > previous.score)
            {
                best = Some(suggestion);
            }
        }
    }
    Ok(best)
}

fn validate_model(info: &Value) -> Result<(), String> {
    if info
        .get("remote_host")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty())
        || info
            .get("remote_model")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    {
        return Err(
            "Choose a downloaded local model; cloud models are not used for highlight suggestions."
                .into(),
        );
    }
    if !info["capabilities"]
        .as_array()
        .is_some_and(|caps| caps.iter().any(|c| c == "vision"))
    {
        return Err("The selected Ollama model cannot read images. Install qwen3-vl:4b-instruct or choose another local vision model.".into());
    }
    Ok(())
}

fn sample_window(
    path: &str,
    media: &MediaInfo,
    is_hdr: bool,
    gamma: f64,
    window: AnalysisWindow,
    cancel: &AtomicBool,
) -> Result<Vec<Sample>, String> {
    let _lease = crate::background::read_media(path);
    let max_height =
        ((1280.0 * media.height as f64 / media.width as f64).floor() as i64).clamp(2, 720);
    let mut decoder = crate::libav::ScrubDecoder::open(
        path,
        media.width,
        media.height,
        is_hdr,
        gamma,
        max_height,
    )
    .ok_or("Could not open this clip for highlight detection.")?;
    let mut samples = Vec::new();
    for time in window.sample_times() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Highlight detection cancelled.".into());
        }
        // A container ends after its final frame's timestamp; seeking into that gap returns EOF.
        let time = time.min((media.duration_sec - 1.0 / media.fps.max(1.0)).max(0.0));
        let (w, h, rgba, realized) = decoder
            .frame_at(time)
            .ok_or_else(|| format!("Could not decode a highlight sample at {time:.1}s."))?;
        let mut png = Vec::new();
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Sub)
            .write_image(&rgba, w, h, ExtendedColorType::Rgba8)
            .map_err(|e| format!("Could not encode a highlight sample: {e}"))?;
        samples.push(Sample {
            time: realized - window.start_sec,
            image: STANDARD.encode(png),
        });
    }
    Ok(samples)
}

async fn post_json(
    client: &reqwest::Client,
    base: &str,
    route: &str,
    body: Value,
) -> Result<Value, String> {
    let response = client.post(format!("{base}/api/{route}")).json(&body).send().await.map_err(|e| {
        if e.is_connect() {
            "Cannot reach Ollama. Start Ollama locally, then run `ollama pull qwen3-vl:4b-instruct` and try again.".into()
        } else if e.is_timeout() {
            "Ollama took longer than five minutes for one section. Try a smaller vision model or a shorter clip.".into()
        } else {
            format!("Ollama request failed: {e}")
        }
    })?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("Invalid Ollama response: {e}"))?;
    if !status.is_success() {
        let detail = value["error"].as_str().unwrap_or("Request failed");
        return Err(format!("Ollama: {detail}. Check the model in Settings → Highlight suggestions; install it with `ollama pull <model>`."));
    }
    Ok(value)
}

fn chat_body(model: &str, query: &str, window: AnalysisWindow, samples: &[Sample]) -> Value {
    let timestamps = samples
        .iter()
        .enumerate()
        .map(|(i, sample)| format!("image {} = {}s", i + 1, sample.time))
        .collect::<Vec<_>>()
        .join(", ");
    json!({
        "model": model,
        "stream": false,
        "think": false,
        "format": detection_schema(),
        // Some local models still emit thinking with think=false; leave room for the final JSON.
        "options": { "temperature": 0, "num_ctx": 40960, "num_predict": 4096 },
        "messages": [{
            "role": "user",
            "content": format!(
                "These images are chronological samples from a gameplay recording, one per second. \
                 This section lasts {} seconds. Image timestamps relative to this section: {timestamps}. \
                 Inspect ALL images before choosing the strongest complete play matching this request: {query}. \
                 Return JSON with event=null if no highlight is visible. \
                 Otherwise return event with startSec, endSec, score, reason. Times must be relative \
                 to THIS section between 0 and {} with endSec >= startSec. Include the whole action sequence from \
                 the first elimination to the last, not just a notification after the event. \
                 Do not add lead-in or aftermath. Score interest consistently: \
                 20 ordinary action, 40 a routine elimination, 60 a routine multi-kill, \
                 80 multiple precise aimed eliminations such as consecutive headshots, \
                 90 a decisive clutch or unusually difficult play, 100 an exceptional play. \
                 Prefer skilled execution over routine or assisted kills even when both show the same multi-kill banner. \
                 Credit only the POV player's distinct eliminations, not teammate kills, assists, \
                 or repeated entries in the kill feed. Persistent cumulative kill-streak banners \
                 do not prove multiple new kills in this section. Do not infer an elimination from aiming or firing alone. \
                 Give a brief reason based only on visible evidence, including what makes this play stand out. \
                 Do not invent off-screen events or audio.",
                window.end_sec - window.start_sec, window.end_sec - window.start_sec
            ),
            "images": samples.iter().map(|s| &s.image).collect::<Vec<_>>()
        }]
    })
}

async fn query_window(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    query: &str,
    window: AnalysisWindow,
    samples: &[Sample],
) -> Result<HighlightDetection, String> {
    #[derive(Deserialize)]
    struct Message {
        content: String,
    }
    #[derive(Deserialize)]
    struct Response {
        message: Message,
        done: bool,
        #[serde(default)]
        done_reason: Option<String>,
    }

    let response = post_json(
        client,
        base,
        "chat",
        chat_body(model, query, window, samples),
    )
    .await?;
    let response: Response =
        serde_json::from_value(response).map_err(|e| format!("Invalid Ollama response: {e}"))?;
    if !response.done {
        return Err("Ollama did not finish analyzing this section. Try again.".into());
    }
    if response.done_reason.as_deref() == Some("length") {
        return Err("Ollama reached its output limit before finishing the highlight answer. Try a shorter clip or choose a local non-thinking vision model in Settings → Highlight suggestions.".into());
    }
    let content = response.message.content.trim();
    if content.is_empty() {
        return Err("Ollama returned an empty answer. Try again or choose another local vision model in Settings → Highlight suggestions.".into());
    }
    serde_json::from_str(content)
        .map_err(|e| format!("The model did not return a valid highlight: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    #[ignore = "requires a local recording, labelled event range, and Ollama"]
    fn local_highlight_covers_expected_event() {
        let path = std::env::var("QLIPQ_TEST_INPUT").expect("set QLIPQ_TEST_INPUT");
        let expected_start: f64 = std::env::var("QLIPQ_HIGHLIGHT_EXPECT_START")
            .unwrap()
            .parse()
            .unwrap();
        let expected_end: f64 = std::env::var("QLIPQ_HIGHLIGHT_EXPECT_END")
            .unwrap()
            .parse()
            .unwrap();
        assert!(expected_end > expected_start);
        let (media, is_hdr) = crate::libav::probe(&path).unwrap();
        let result = runtime()
            .block_on(detect(
                HighlightRequest {
                    path,
                    media,
                    is_hdr,
                    gamma: 1.0,
                    model: qlipq_core::config::AppConfig::default().highlight_model,
                    query: "A multi-kill, clutch, impressive play, or funny gameplay moment".into(),
                },
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicUsize::new(0)),
            ))
            .unwrap()
            .unwrap();
        eprintln!("Selected highlight: {result:?}");
        assert!(result.trim.start_sec <= expected_start, "{result:?}");
        assert!(result.trim.end_sec >= expected_end, "{result:?}");
        assert!(
            result.trim.end_sec - result.trim.start_sec <= expected_end - expected_start + 10.0,
            "{result:?}"
        );
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn empty_answer_at_output_limit_reports_the_cause_instead_of_json_eof() {
        let (url, server) = mock_ollama(1, |_, _, _| {
            (
                200,
                json!({
                    "done": true, "done_reason": "length", "eval_count": 512,
                    "message": {"role": "assistant", "content": "", "thinking": "Still analyzing the frames"}
                }),
            )
        });
        let error = runtime()
            .block_on(query_window(
                &client().unwrap(),
                &url,
                "qwen3-vl:4b",
                "a highlight",
                AnalysisWindow {
                    start_sec: 0.0,
                    end_sec: 1.0,
                },
                &[],
            ))
            .unwrap_err();
        server.join().unwrap();
        assert!(error.contains("output limit"), "{error}");
        assert!(!error.contains("EOF"), "{error}");
    }

    #[test]
    fn rejects_empty_and_truncated_answers_without_using_thinking_as_a_highlight() {
        for (reason, content, expected) in [
            ("stop", "", "empty answer"),
            ("stop", " \n\t", "empty answer"),
            ("length", "{\"event\":", "output limit"),
            ("length", "{\"event\":null}", "output limit"),
        ] {
            let (url, server) = mock_ollama(1, move |_, _, _| {
                (
                    200,
                    json!({
                        "done": true, "done_reason": reason,
                        "message": {"content": content, "thinking": "{\"event\":null}"}
                    }),
                )
            });
            let error = runtime()
                .block_on(query_window(
                    &client().unwrap(),
                    &url,
                    "test",
                    "action",
                    AnalysisWindow {
                        start_sec: 0.0,
                        end_sec: 1.0,
                    },
                    &[],
                ))
                .unwrap_err();
            server.join().unwrap();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("EOF"), "{error}");
        }
    }

    pub(crate) fn mock_ollama(
        count: usize,
        respond: impl Fn(usize, &str, Value) -> (u16, Value) + Send + 'static,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for index in 0..count {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                let (header_end, length) = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while request.len() < header_end + length {
                    let n = stream.read(&mut buffer).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buffer[..n]);
                }
                let header = String::from_utf8_lossy(&request[..header_end]);
                let body =
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap();
                let (status, response) = respond(index, &header, body);
                let body = response.to_string();
                let _ = write!(stream, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        (url, worker)
    }

    fn request(path: String) -> HighlightRequest {
        HighlightRequest {
            path,
            media: MediaInfo {
                duration_sec: 31.0,
                width: 64,
                height: 36,
                fps: 2.0,
                video_codec: "rawvideo".into(),
                audio_streams: Vec::new(),
                size_bytes: None,
                encoder: None,
            },
            is_hdr: false,
            gamma: 1.0,
            model: "qwen3-vl:4b-instruct".into(),
            query: "a multi-kill".into(),
        }
    }

    #[test]
    fn samples_real_video_and_selects_the_best_event_across_windows() {
        let path = std::env::temp_dir().join(format!("qlipq-highlight-{}.y4m", std::process::id()));
        let mut video = b"YUV4MPEG2 W64 H36 F2:1 Ip A1:1 C420jpeg\n".to_vec();
        for frame in 0..62u8 {
            video.extend_from_slice(b"FRAME\n");
            video.extend(std::iter::repeat_n(32 + frame, 64 * 36));
            video.extend(std::iter::repeat_n(128, 64 * 36 / 2));
        }
        std::fs::write(&path, video).unwrap();
        let (url, server) = mock_ollama(3, |index, headers, body| {
            assert_eq!(body["model"], "qwen3-vl:4b-instruct");
            if index == 0 {
                assert!(headers.starts_with("POST /api/show "));
                return (200, json!({"capabilities": ["vision", "completion"]}));
            }
            assert!(headers.starts_with("POST /api/chat "));
            assert_eq!(body["stream"], false);
            let images = body["messages"][0]["images"].as_array().unwrap();
            assert_eq!(images.len(), if index == 1 { 30 } else { 6 });
            // Measured Qwen3-VL image cost includes delimiters. Reserve the complete output budget
            // plus prompt overhead, so thinking cannot consume the space needed for the final JSON.
            let output_budget = body["options"]["num_predict"].as_u64().unwrap();
            assert!(output_budget >= 4096);
            assert!(
                body["options"]["num_ctx"].as_u64().unwrap()
                    >= images.len() as u64 * 1100 + 2048 + output_budget
            );
            let png = STANDARD.decode(images[0].as_str().unwrap()).unwrap();
            let image = image::load_from_memory(&png).unwrap();
            assert_eq!((image.width(), image.height()), (64, 36));
            let prompt = body["messages"][0]["content"].as_str().unwrap();
            assert!(prompt.contains("image 1 = 0.5s"));
            assert!(prompt.contains("a multi-kill"));
            let event = if index == 1 {
                json!({"startSec": 10, "endSec": 11, "score": 60, "reason": "First event"})
            } else {
                json!({"startSec": 1, "endSec": 3, "score": 90, "reason": "Better event"})
            };
            (
                200,
                json!({"done": true, "message": {"content": json!({"event": event}).to_string()}}),
            )
        });
        let section = Arc::new(AtomicUsize::new(0));
        let result = runtime().block_on(detect_at(
            request(path.to_string_lossy().into_owned()),
            Arc::new(AtomicBool::new(false)),
            section.clone(),
            &url,
        ));
        let _ = std::fs::remove_file(path);
        let suggestion = result.unwrap().unwrap();
        server.join().unwrap();
        assert_eq!(
            (suggestion.trim.start_sec, suggestion.trim.end_sec),
            (23.0, 30.0)
        );
        assert_eq!(suggestion.reason, "Better event");
        assert_eq!(section.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn handles_no_highlight_malformed_output_and_missing_model() {
        for (status, response, expected) in [
            (
                200,
                json!({"done": true, "message": {"content": "{\"event\":null}"}}),
                None,
            ),
            (
                200,
                json!({"done": true, "message": {"content": "not JSON"}}),
                Some("valid highlight"),
            ),
            (
                200,
                json!({"done": false, "message": {"content": "{}"}}),
                Some("did not finish"),
            ),
            (
                404,
                json!({"error": "model not found"}),
                Some("ollama pull"),
            ),
        ] {
            let (url, server) = mock_ollama(1, move |_, _, _| (status, response.clone()));
            let result = runtime().block_on(query_window(
                &client().unwrap(),
                &url,
                "test",
                "action",
                AnalysisWindow {
                    start_sec: 0.0,
                    end_sec: 5.0,
                },
                &[],
            ));
            server.join().unwrap();
            match expected {
                Some(error) => assert!(result.unwrap_err().contains(error)),
                None => assert!(result.unwrap().event.is_none()),
            }
        }
    }

    #[test]
    fn preserves_hud_detail_in_highlight_samples() {
        let path =
            std::env::temp_dir().join(format!("qlipq-highlight-hud-{}.y4m", std::process::id()));
        let mut video = b"YUV4MPEG2 W1280 H720 F2:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        video.extend(std::iter::repeat_n(128, 1280 * 720 * 3 / 2));
        std::fs::write(&path, video).unwrap();
        let mut request = request(path.to_string_lossy().into_owned());
        request.media.width = 1280;
        request.media.height = 720;
        request.media.duration_sec = 0.5;
        let samples = sample_window(
            &request.path,
            &request.media,
            false,
            1.0,
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 0.5,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        let _ = std::fs::remove_file(path);
        let png = STANDARD.decode(&samples[0].image).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (1280, 720));
    }

    #[test]
    fn samples_a_single_frame_clip_without_seeking_past_its_last_frame() {
        let path =
            std::env::temp_dir().join(format!("qlipq-highlight-short-{}.y4m", std::process::id()));
        let mut video = b"YUV4MPEG2 W64 H36 F2:1 Ip A1:1 C420jpeg\nFRAME\n".to_vec();
        video.extend(std::iter::repeat_n(128, 64 * 36 * 3 / 2));
        std::fs::write(&path, video).unwrap();
        let mut request = request(path.to_string_lossy().into_owned());
        request.media.duration_sec = 0.5;
        let result = sample_window(
            &request.path,
            &request.media,
            false,
            1.0,
            AnalysisWindow {
                start_sec: 0.0,
                end_sec: 0.5,
            },
            &AtomicBool::new(false),
        );
        let _ = std::fs::remove_file(path);
        let samples = result.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].time, 0.0);
    }

    #[test]
    fn cancellation_interrupts_an_in_flight_ollama_request() {
        let token = Arc::new(AtomicBool::new(false));
        let cancel = token.clone();
        let (url, server) = mock_ollama(1, move |_, _, _| {
            cancel.store(true, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(300));
            (200, json!({"capabilities": ["vision"]}))
        });
        let result = runtime().block_on(detect_at(
            request("unused.y4m".into()),
            token,
            Arc::new(AtomicUsize::new(0)),
            &url,
        ));
        assert_eq!(result.unwrap_err(), "Highlight detection cancelled.");
        server.join().unwrap();
    }

    #[test]
    fn dropping_or_replacing_a_job_invalidates_its_result() {
        let job = HighlightJob::new(31.0);
        assert_eq!(job.total, 2);
        let token = job.cancel.clone();
        assert!(job.matches(&token));
        assert!(!HighlightJob::new(31.0).matches(&token));
        drop(job);
        assert!(token.load(Ordering::Relaxed));
    }

    #[test]
    fn only_local_vision_models_are_accepted() {
        assert!(validate_model(&json!({"capabilities": ["completion", "vision"]})).is_ok());
        assert!(validate_model(&json!({"capabilities": ["completion"]})).is_err());
        assert!(validate_model(
            &json!({"capabilities": ["vision"], "remote_host": "https://ollama.com"})
        )
        .is_err());
    }
}
