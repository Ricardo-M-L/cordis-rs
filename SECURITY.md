# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.1.x   | ✅        |

## Reporting a Vulnerability

Please report security vulnerabilities to **security@cordis-rs.dev** or open a
[private vulnerability report](https://github.com/Ricardo-M-L/cordis-rs/security/advisories/new)
on GitHub.

We will acknowledge receipt within 48 hours and aim to respond with a fix or
mitigation plan within 7 days.

Do **not** file a public issue for security vulnerabilities.

## Security Best Practices

- Keep `cordis-core` and all dependencies up to date
- Use `cargo audit` to check for known vulnerabilities
- Enable Rust's `-D warnings` flag in CI to catch potential issues early
