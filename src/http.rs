use std::{collections::HashMap, io::Read};

#[derive(Debug)]
enum HttpIterMode {
    Code,
    Msg,
    Header,
    Body,
    Error,
}
pub struct HttpParser {
    combined: Vec<u8>,
    parsed: HttpParsed,
    pos: usize,
    content_length: Option<usize>,
    end_headers: bool,
    error: bool,
    read_body: bool, // setting for whether body should also be read to the end
}
pub struct HttpParsed {
    pub code: Option<i32>,
    pub msg: Option<String>,
    pub headers: HashMap<String, String>,
    pub body: Option<String>,
}

enum HttpIterError {
    EndOfString,
    Fatal(String),
}
fn iter_expect<U, T>(iter: T, value: &str) -> Result<std::iter::Skip<T>, HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone {
    let (iter, next) = iter_extract(iter, value.len())?;
    if next != value {
        return Err(HttpIterError::Fatal(String::from("Comparison failed")));
    }
    return Ok(iter);
}
fn iter_expect_func<U, T, V>(iter: T, char_count: usize, expect: V) -> Result<(std::iter::Skip<T>, String), HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone,
              V: Fn(&String) -> Result<(), String> {
    let (iter, next) = iter_extract(iter, char_count)?;
    expect(&next).map_err(|err| HttpIterError::Fatal(err))?;
    return Ok((iter, next));
}
fn iter_expect_each_char<U, T, V>(iter: T, char_count: usize, expect: V) -> Result<(std::iter::Skip<T>, String), HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone,
              V: Fn(char) -> Result<(), String> {
    let next = iter.clone().take(char_count).map(|(_, c)| c);
    let mut out = String::new();
    for c in next {
        out.push(c);
        expect(c).map_err(|err| HttpIterError::Fatal(err))?;
    }
    if out.len() < char_count {
        return Err(HttpIterError::EndOfString);
    }
    let iter = iter.skip(char_count);
    return Ok((iter, out));
}

fn iter_extract<U, T>(iter: T, char_count: usize) -> Result<(std::iter::Skip<T>, String), HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone {
    let next: String = iter.clone().take(char_count).map(|(_, c)| c).collect();
    if next.len() < char_count {
        return Err(HttpIterError::EndOfString);
    }
    let iter = iter.skip(char_count);
    return Ok((iter, next));
}

fn iter_extract_to_eol<U, T>(iter: T) -> Result<(std::iter::Skip<T>, String), HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone {
    let next = iter.clone().map(|(_, c)| c);
    let mut out: String = String::new();
    for c in next {
        out.push(c);
        if out.ends_with("\r\n") {
            let iter = iter.skip(out.len());
            return Ok((iter, out[0..out.len() - 2].to_string()));
        }
    }
    return Err(HttpIterError::EndOfString);
}
fn iter_extract_header_pair<U, T>(iter: T) -> Result<(std::iter::Skip<T>, Option<(String, String)>), HttpIterError>
        where T: Iterator<Item = (U, char)> + Clone {
    let (iter, line) = iter_extract_to_eol(iter)?;
    let pair = line.split_once(": ")
        .map(|(key, val)| (key.to_string(), val.to_string()));
    if let Some(pair) = pair {
        return Ok((iter, Some(pair)));
    } else if line.len() == 0 {
        return Ok((iter, None));
    } else {
        return Err(HttpIterError::Fatal(format!("Invalid HTTP header: {}", line)));
    }
}

enum HttpIterStride {
    Stride(usize),
    End
}
impl HttpIterStride {
    fn to_stride<U, T>(mut iter: T) -> Self
            where T: Iterator<Item = (usize, U)> {
        return match iter.next() {
            Some((stride, _)) => HttpIterStride::Stride(stride),
            None => HttpIterStride::End,
        }
    }
}

impl Iterator for HttpParser {
    type Item = Result<(), String>;

    fn next(&mut self) -> Option<Result<(), String>> {
        let buf = String::from_utf8_lossy(&self.combined[self.pos..]);
        let mode =
            if self.error { HttpIterMode::Error }
            else if self.parsed.code.is_none() { HttpIterMode::Code }
            else if self.parsed.msg.is_none() { HttpIterMode::Msg }
            else if !self.end_headers { HttpIterMode::Header }
            else if self.read_body && self.parsed.body.is_none() { HttpIterMode::Body }
            else { return None };
        let iter = Ok(buf.chars().enumerate());
        let stride = match mode {
            HttpIterMode::Code => iter
                    .and_then(|iter| iter_expect(iter, "HTTP/1.1 "))
                    .and_then(|iter| iter_expect_each_char(iter, 3,
                               |c| ('0' <= c && c <= '9')
                                   .then_some(()).ok_or("HTTP code must be 3 digits".to_string()))
                              )
                    .and_then(|(iter, parsed)| iter_expect(iter, " ")
                              .map(|iter| (iter, parsed)))
                    .and_then(|(iter, code)| {
                        let code = i32::from_str_radix(&code, 10)
                            .or(Err(HttpIterError::Fatal(format!(""))))?;
                        self.parsed.code = Some(code);
                        Ok(HttpIterStride::to_stride(iter))
                    }),
            HttpIterMode::Msg => iter
                    .and_then(|iter| iter_extract_to_eol(iter))
                    .and_then(|(iter, msg)| {
                        self.parsed.msg = Some(msg);
                        Ok(HttpIterStride::to_stride(iter))
                    }),
            HttpIterMode::Header => iter
                    .and_then(|iter| iter_extract_header_pair(iter))
                    .and_then(|(iter, pair)| {
                        if let Some((key, value)) = pair {
                            self.parsed.headers.insert(key.to_lowercase(), value);
                        } else {
                            self.end_headers = true;
                            // check if we have enough info to predict body length
                            if !self.read_body {
                                // do nothing, body length does not have to be known
                            } else if let Some(_) = self.parsed.headers.get("transfer-encoding") {
                                todo!()
                            } else if let Some(length) = self.parsed.headers.get("content-length") {
                                self.content_length = Some(usize::from_str_radix(length, 10)
                                    .map_err(|_| HttpIterError::Fatal(format!("Error parsing content length: {}", length)))?);
                            } else {
                                return Err(HttpIterError::Fatal("Missing content length specifier".to_string()));
                            }
                        }
                        Ok(HttpIterStride::to_stride(iter))
                    }),
            HttpIterMode::Body => iter
                    .and_then(|iter| {
                        let len = self.content_length.ok_or(HttpIterError::Fatal("Missing content length".to_string()))?;
                        let body = iter.map(|(_, c)| c).collect::<String>();
                        if body.len() < len {
                            return Err(HttpIterError::EndOfString)
                        }
                        self.parsed.body = Some(body[0..len].to_string());
                        Ok(HttpIterStride::End)
                    }),
            HttpIterMode::Error => return None,
        };
        match stride {
            Ok(stride) => Some(Ok(match stride {
                HttpIterStride::Stride(stride) => self.pos += stride,
                HttpIterStride::End => self.pos += buf.len(),
            })),
            Err(HttpIterError::EndOfString) => return None,
            Err(HttpIterError::Fatal(msg)) => {
                self.error = true;
                return Some(Err(format!("Error parsing HTTP: {}", msg)))
            },
        }
    }
}
impl HttpParser {
    pub fn init(read_body: bool) -> Self {
        Self {
            combined: vec![],
            pos: 0,
            content_length: None,
            parsed: HttpParsed {
                code: None,
                msg: None,
                headers: HashMap::new(),
                body: None,
            },
            end_headers: false,
            error: false,
            read_body,
        }
    }
    pub fn parse(&mut self, buf: &[u8]) -> std::result::Result<(), String> {
        self.combined.extend_from_slice(&buf);
        for res in self {
            res?
        }
        Ok(())
    }
    pub fn eof(&mut self) -> std::result::Result<(), String> {
        self.error = true;
        Err("eof unexpected".to_string())
    }
    pub fn should_continue(&self) -> bool {
        if self.read_body {
            return !self.error && self.parsed.body.is_none();
        } else {
            return !self.error && !self.end_headers;
        }
    }
}
impl Into<HttpParsed> for HttpParser {
    fn into(self) -> HttpParsed {
        return self.parsed
    }
}
impl HttpParsed {
    fn read<T: Read, const BUF_SIZE: usize>(stream: &mut T, read_body: bool) -> Self {
        let mut buf = [0u8; BUF_SIZE];
        let mut http = HttpParser::init(read_body);
        'outer: while http.should_continue() {
            match stream.read(&mut buf) {
                Ok(n) => {
                    http.parse(&buf[0..n]).unwrap();
                },
                Err(err) => match err.kind() {
                    std::io::ErrorKind::UnexpectedEof => {
                        http.eof().unwrap();
                        break 'outer;
                    },
                    _ => panic!("{}", err),
                },
            }
            buf.fill(0);
        }
        http.into()
    }

    pub fn read_to_end<T: Read, const BUF_SIZE: usize>(stream: &mut T) -> Self {
        Self::read::<_, BUF_SIZE>(stream, true)
    }
    pub fn read_headers<T: Read, const BUF_SIZE: usize>(stream: &mut T) -> Self {
        Self::read::<_, BUF_SIZE>(stream, false)
    }
}
