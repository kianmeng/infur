#![allow(dead_code)]

/// Describes one video stream
#[derive(Debug, Clone, PartialEq)]
struct Stream {
    num: u32,
    width: u32,
    height: u32,
    fps: Option<f32>,
}

/// Describes one output's video stream
#[derive(Debug, Clone, PartialEq)]
struct InputStream {
    from: String,
    stream: Stream,
}

/// Describes one output's video stream
#[derive(Debug, Clone, PartialEq)]
struct OutputStream {
    to: String,
    stream: Stream,
}

/// Describes a stream's update
#[derive(Debug, Clone, PartialEq)]
struct FrameUpdate {
    frame: u64,
    fps: Option<f32>,
    dup: Option<u32>,
    drop: Option<u32>,
}

//#[derive(Debug, Clone, PartialEq)]
//struct DecoderUpdate {}

#[derive(Debug, Clone, PartialEq)]
enum VideoInfo {
    Input(InputStream),
    Output(OutputStream),
    Frame(FrameUpdate),
    //    Decoder(DecoderUpdate),
}

#[derive(Debug, Clone, PartialEq)]
enum ParseContext {
    Stateless,
    Output(u32, String),
    Input(u32, String),
}

#[derive(Debug, Clone)]
struct InfoParser {
    mode: ParseContext,
}

#[derive(Debug, Clone, PartialEq)]
struct ParseError {
    context: ParseContext,
    line: String,
    reason: String,
}

type InfoResult = Result<Option<VideoInfo>, ParseError>;
type FilteredInfo = Result<VideoInfo, ParseError>;

impl InfoParser {
    fn default() -> Self {
        InfoParser { mode: ParseContext::Stateless }
    }

    fn error_on(&self, reason: impl Into<String>, line: &str) -> ParseError {
        ParseError { context: self.mode.clone(), line: line.to_string(), reason: reason.into() }
    }

    fn push(&mut self, line: &str) -> InfoResult {
        let error_on = |reason| self.error_on(reason, line);
        let error_on_ = |reason| self.error_on(reason, line); // no generic closures

        // Begin Stream
        let output = line.strip_prefix("Output #").unwrap_or(line);
        let input = line.strip_prefix("Input #").unwrap_or(line);
        let in_out = match (input.len(), output.len()) {
            (i, _) if i < line.len() => Some((true, input)),
            (_, o) if o < line.len() => Some((false, output)),
            _ => None,
        };
        if let Some((is_input, remaining)) = in_out {
            let mut parts = remaining.split(',');
            let num_stream = parts
                .next()
                .ok_or_else(|| error_on("no delimiter after output number"))?
                .trim()
                .parse::<u32>()
                .map_err(|e| error_on_(format!("# not a number but {:?}", e)))?;
            let to_from =
                parts.last().ok_or_else(|| error_on("no last stream element (from or to)"))?.trim();

            // unquote and extract if possible
            let to_from =
                to_from.strip_prefix(if is_input { "from '" } else { "to '" }).unwrap_or(to_from);
            let to_from = to_from.strip_suffix("':").unwrap_or(to_from);

            self.mode = if is_input {
                ParseContext::Input(num_stream, to_from.to_string())
            } else {
                ParseContext::Output(num_stream, to_from.to_string())
            };
            return Ok(None);
        }

        let line_trimmed = line.trim();
        let frame_str = line_trimmed.strip_prefix("frame=").unwrap_or(line_trimmed);

        // reset if some other header comes up
        if line_trimmed.len() == line.len() && !frame_str.len() < line.len() {
            self.mode = ParseContext::Stateless;
            return Ok(None);
        }

        // VideoInfos
        let stream_str = line_trimmed.strip_prefix("Stream #").unwrap_or(line_trimmed);
        if !matches!(self.mode, ParseContext::Stateless) && stream_str.len() < line_trimmed.len() {
            // let chains ftw: https://github.com/rust-lang/rust/issues/53667
            let (is_input, num_stream, to_from) = match self.mode {
                ParseContext::Input(num_stream, ref from) => (true, num_stream, from),
                ParseContext::Output(num_stream, ref to) => (false, num_stream, to),
                _ => return Err(error_on("found Stream while not parsing it")),
            };
            let mut parts = stream_str.split(':');
            let parse_num_stream = parts
                .next()
                .ok_or_else(|| error_on("no delimiter after stream number"))?
                .parse::<u32>()
                .map_err(|e| error_on_(format!("Stream # not a number {:?}", e)))?;

            if num_stream != parse_num_stream {
                return Err(error_on_(format!("Stream {} didn't match Output", parse_num_stream)));
            };

            let mut is_video = false;
            let mut width_height = None;
            let mut fps = None;
            for p in parts {
                if !is_video && p.trim() == "Video" {
                    is_video = true;
                }
                if is_video {
                    for key_vals in p.split(',') {
                        let key_vals = key_vals.trim();
                        let fps_vals = key_vals.trim_end_matches(" fps");
                        if fps_vals.len() < key_vals.len() {
                            fps = fps_vals
                                .parse::<f32>()
                                .map_err(|_| error_on("fps not a number"))?
                                .into();
                        } else {
                            let mut dim_vals = key_vals.splitn(2, 'x');
                            if let (Some(width_str), Some(height_str)) =
                                (dim_vals.next(), dim_vals.next())
                            {
                                let height_str =
                                    height_str.split_once(' ').map_or_else(|| "", |v| v.0);
                                if let (Ok(w), Ok(h)) =
                                    (width_str.parse::<u32>(), height_str.parse::<u32>())
                                {
                                    width_height = Some((w, h))
                                };
                            }
                        }
                    }
                }
            }
            return if let Some((width, height)) = width_height {
                let stream = Stream { num: num_stream, width, height, fps };
                let info = if is_input {
                    VideoInfo::Input(InputStream { from: to_from.clone(), stream })
                } else {
                    VideoInfo::Output(OutputStream { to: to_from.clone(), stream })
                };
                self.mode = ParseContext::Stateless;
                Ok(Some(info))
            } else {
                Err(error_on("didn't find <width>x<height> in first video Stream Output"))
            };
        }

        // Frame message
        if frame_str.len() < line_trimmed.len() {
            // frame required
            if let Some((frame_num_str, mut frame_rest)) = frame_str.trim().split_once(' ') {
                let frame = frame_num_str
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| error_on("frame is no number"))?;

                // other key values if available
                let (mut fps, mut dup, mut drop) = (None, None, None);
                while let Some((key, rest)) = frame_rest.split_once('=') {
                    if let Some((value, rest)) = rest.trim_start().split_once(' ') {
                        match key {
                            "fps" => fps = value.parse().ok(),
                            "dup" => dup = value.parse().ok(),
                            "drop" => drop = value.parse().ok(),
                            _ => {}
                        }
                        frame_rest = rest;
                    } else {
                        frame_rest = rest;
                    }
                }
                let frame_upd = FrameUpdate { frame, fps, dup, drop };
                Ok(Some(VideoInfo::Frame(frame_upd)))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn iter_on<'a, I>(&'a mut self, lines: I) -> impl Iterator<Item = FilteredInfo> + 'a
    where
        I: IntoIterator<Item = &'a str> + 'a,
    {
        fn un_opt(info: InfoResult) -> Option<FilteredInfo> {
            match info {
                Err(e) => Some(Err(e)),
                Ok(Some(m)) => Some(Ok(m)),
                Ok(None) => None,
            }
        }

        lines.into_iter().map(|l| self.push(l)).filter_map(un_opt)
    }
}

#[cfg(test)]
mod test {
    use crate::parse::{FrameUpdate, Stream};

    use super::{InfoParser, InputStream, OutputStream, VideoInfo};

    static TEST_INFO: &str = r#"Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'media/huhu_test.mp4':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomiso2avc1mp41
    title           : Session streamed with GStreamer
    encoder         : Lavf58.45.100
    comment         : rtsp-server
  Duration: 00:29:58.68, start: 0.000000, bitrate: 650 kb/s
  Stream #0:0(und): Video: h264 (Main) (avc1 / 0x31637661), yuvj420p(pc, bt709), 1280x720 [SAR 1:1 DAR 16:9], 647 kb/s, 29.59 fps, 30 tbr, 90k tbn, 180k tbc (default)
Metadata:
  handler_name    : VideoHandler
  vendor_id       : [0][0][0][0]
Stream mapping:
  Stream #0:0 -> #0:0 (h264 (native) -> rawvideo (native))
Press [q] to stop, [?] for help
[swscaler @ 0x7fb0ac4dc000] deprecated pixel format used, make sure you did set range correctly
Output #0, image2pipe, to 'pipe:':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomiso2avc1mp41
    title           : Session streamed with GStreamer
    comment         : rtsp-server
        encoder         : Lavf58.76.100
  Stream #0:0(und): Video: rawvideo (BGR[24] / 0x18524742), bgr24(pc, gbr/bt709/bt709, progressive), 1280x720 [SAR 1:1 DAR 16:9], q=2-31, 663552 kb/s, 30 fps, 30 tbn (default)
    Metadata:
      handler_name    : VideoHandler
      vendor_id       : [0][0][0][0]
      encoder         : Lavc58.134.100 rawvideo

frame= 3926 fps=978 q=-0.0 size=10600200kB time=00:02:10.86 bitrate=663552.0kbits/s speed=32.6x
frame= 4026 fps=1002 q=-0.0 size=10870200kB time=00:02:14.20 bitrate=663552.0kbits/s speed=33.4x
frame=27045 fps= 1019.6 q=-0.0 size=73021500kB time=00:15:01.50 bitrate=663552.0kbits/s dup= 0 drop=5 speed=  34x"#;

    #[test]
    fn test_parse_info() {
        let mut parser = InfoParser::default();
        let mut infos = parser.iter_on(TEST_INFO.lines());

        // input
        assert_eq!(
            infos.next().unwrap(),
            Ok(VideoInfo::Input(InputStream {
                stream: Stream { num: 0, width: 1280, height: 720, fps: Some(29.59f32) },
                from: "media/huhu_test.mp4".to_string(),
            }))
        );
        assert_eq!(
            infos.next().unwrap(),
            Ok(VideoInfo::Output(OutputStream {
                stream: Stream { num: 0, width: 1280, height: 720, fps: Some(30f32) },
                to: "pipe:".to_string(),
            }))
        );
        // frames
        assert_eq!(
            infos.next().unwrap(),
            Ok(VideoInfo::Frame(FrameUpdate {
                frame: 3926,
                fps: Some(978f32),
                dup: None,
                drop: None,
            }))
        );
        assert_eq!(
            infos.next().unwrap(),
            Ok(VideoInfo::Frame(FrameUpdate {
                frame: 4026,
                fps: Some(1002f32),
                dup: None,
                drop: None,
            }))
        );
        assert_eq!(
            infos.next().unwrap(),
            Ok(VideoInfo::Frame(FrameUpdate {
                frame: 27045,
                fps: Some(1019.6f32),
                dup: Some(0),
                drop: Some(5),
            }))
        );
    }
}
