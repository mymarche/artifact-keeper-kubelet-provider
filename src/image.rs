//! Registry host of an image reference, following the Docker reference rule:
//! the first path component names a registry when it contains `.` or `:`, or
//! is `localhost`; otherwise the image is on Docker Hub.

use crate::error::Error;

const DOCKER_HUB: &str = "docker.io";

pub fn registry_host(image: &str) -> Result<&str, Error> {
    let image = image.trim();
    if image.is_empty() {
        return Err(Error::Usage("request has an empty image".into()));
    }
    // A digest may contain ':' but never '/', so it cannot be mistaken for a
    // host; strip it anyway so the rule below only ever sees name components.
    let name = image.split_once('@').map_or(image, |(name, _)| name);
    match name.split_once('/') {
        Some((first, _)) if first.contains('.') || first.contains(':') || first == "localhost" => {
            Ok(first)
        }
        _ => Ok(DOCKER_HUB),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_host_table() {
        let cases = [
            ("ak.example.com/payments/api:1.2", "ak.example.com"),
            ("ak.example.com/payments/api", "ak.example.com"),
            ("ak.example.com:5000/team/app:1", "ak.example.com:5000"),
            (
                "ak.example.com:5000/team/app@sha256:0123456789abcdef",
                "ak.example.com:5000",
            ),
            (
                "ak.example.com/team/app:1.0@sha256:0123456789abcdef",
                "ak.example.com",
            ),
            ("localhost/app", "localhost"),
            ("localhost:5000/app:dev", "localhost:5000"),
            ("[::1]:5000/app", "[::1]:5000"),
            ("10.0.0.5:8443/repo/app", "10.0.0.5:8443"),
            ("busybox", "docker.io"),
            ("busybox:1.36", "docker.io"),
            ("library/busybox:1.36", "docker.io"),
            ("busybox@sha256:0123456789abcdef", "docker.io"),
        ];
        for (image, host) in cases {
            assert_eq!(registry_host(image).unwrap(), host, "image {image}");
        }
    }

    #[test]
    fn empty_image_is_refused() {
        assert!(registry_host("  ").is_err());
    }
}
