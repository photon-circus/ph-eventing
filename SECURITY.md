# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅        |

## Reporting a Vulnerability

If you discover a security vulnerability in ph-eventing, **please do not open a
public issue.**

Instead, report it privately by emailing **steve@giacomelli.ca** with:

- A description of the vulnerability.
- Steps to reproduce or a minimal proof-of-concept.
- The impact you believe it has.

You should receive an acknowledgement within **72 hours**. The maintainer will
work with you to understand the issue and coordinate a fix before any public
disclosure.

## Scope

ph-eventing is a `#![no_std]` library with no network, filesystem, or OS
interaction. Security-relevant concerns are primarily:

- **Memory safety** — unsound `unsafe` blocks, torn reads, data races.
- **Denial of service** — unbounded loops or panics in library code on
  well-formed input.

## Disclosure

Once a fix is available, a security advisory will be published via
[GitHub Security Advisories](https://github.com/photon-circus/ph-eventing/security/advisories)
and the fix will be released as a patch version.
