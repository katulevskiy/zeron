//! A minimal `multipart/form-data` body (the Worker upload). Built by hand so
//! the bytes are exactly what the tests assert.

pub struct Multipart {
    boundary: String,
    body: Vec<u8>,
}

impl Default for Multipart {
    fn default() -> Self {
        Self::new()
    }
}

impl Multipart {
    pub fn new() -> Self {
        let mut bytes = [0u8; 12];
        getrandom::getrandom(&mut bytes).expect("the OS random source is unavailable");
        Self::with_boundary(format!("zeron-{}", hex::encode(bytes)))
    }

    pub fn with_boundary(boundary: impl Into<String>) -> Self {
        Self {
            boundary: boundary.into(),
            body: Vec::new(),
        }
    }

    /// Add a part. `filename` makes it a file part (the Workers API names
    /// module parts by their file name).
    pub fn part(
        &mut self,
        name: &str,
        filename: Option<&str>,
        content_type: &str,
        bytes: &[u8],
    ) -> &mut Self {
        self.body
            .extend_from_slice(format!("--{}\r\n", self.boundary).as_bytes());
        let disposition = match filename {
            Some(file) => format!("form-data; name=\"{name}\"; filename=\"{file}\""),
            None => format!("form-data; name=\"{name}\""),
        };
        self.body.extend_from_slice(
            format!("Content-Disposition: {disposition}\r\nContent-Type: {content_type}\r\n\r\n")
                .as_bytes(),
        );
        self.body.extend_from_slice(bytes);
        self.body.extend_from_slice(b"\r\n");
        self
    }

    /// `(content-type header, body)`.
    pub fn finish(mut self) -> (String, Vec<u8>) {
        self.body
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        (
            format!("multipart/form-data; boundary={}", self.boundary),
            self.body,
        )
    }
}

/// One parsed part (tests and the mock server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

/// Parse a `multipart/form-data` body given its content-type header.
pub fn parse(content_type: &str, body: &[u8]) -> Option<Vec<Part>> {
    let boundary = content_type
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))?
        .trim_matches('"');
    let delimiter = format!("--{boundary}");
    let text = body;
    let mut parts = Vec::new();
    let mut rest = text;
    // Skip to the first delimiter.
    let first = find(rest, delimiter.as_bytes())?;
    rest = &rest[first + delimiter.len()..];
    loop {
        if rest.starts_with(b"--") {
            return Some(parts);
        }
        rest = rest.strip_prefix(b"\r\n")?;
        let end = find(rest, format!("\r\n{delimiter}").as_bytes())?;
        let chunk = &rest[..end];
        rest = &rest[end + 2 + delimiter.len()..];
        let split = find(chunk, b"\r\n\r\n")?;
        let headers = std::str::from_utf8(&chunk[..split]).ok()?;
        let mut part = Part {
            name: String::new(),
            filename: None,
            content_type: None,
            body: chunk[split + 4..].to_vec(),
        };
        for line in headers.split("\r\n") {
            let (key, value) = line.split_once(':')?;
            match key.trim().to_ascii_lowercase().as_str() {
                "content-disposition" => {
                    for attr in value.split(';').map(str::trim) {
                        if let Some(v) = attr.strip_prefix("name=") {
                            part.name = v.trim_matches('"').to_string();
                        } else if let Some(v) = attr.strip_prefix("filename=") {
                            part.filename = Some(v.trim_matches('"').to_string());
                        }
                    }
                }
                "content-type" => part.content_type = Some(value.trim().to_string()),
                _ => {}
            }
        }
        parts.push(part);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_parses() {
        let mut form = Multipart::with_boundary("b0");
        form.part("metadata", None, "application/json", b"{\"a\":1}")
            .part(
                "worker.js",
                Some("worker.js"),
                "application/javascript+module",
                b"export default {}",
            );
        let (content_type, body) = form.finish();
        assert_eq!(content_type, "multipart/form-data; boundary=b0");
        assert_eq!(
            String::from_utf8(body.clone()).unwrap(),
            "--b0\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json\r\n\r\n{\"a\":1}\r\n\
             --b0\r\nContent-Disposition: form-data; name=\"worker.js\"; filename=\"worker.js\"\r\nContent-Type: application/javascript+module\r\n\r\nexport default {}\r\n\
             --b0--\r\n"
        );
        let parts = parse(&content_type, &body).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].filename.as_deref(), Some("worker.js"));
        assert_eq!(parts[1].body, b"export default {}");
    }
}
