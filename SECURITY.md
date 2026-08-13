# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.3.x   | ✅        |
| < 0.3   | ❌ — upgrade; fixes land on the latest minor only |

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

- **Memory safety** — unsound `unsafe` blocks, torn reads, data races, in any
  of the concurrent primitives (`SeqRing`, `EventBuf`, `LatestBuf`,
  `EventFlags`, `CountedSignal`) or the `MaybeUninit` handling in
  `Block`/`BlockBuilder` and `RingBuf`.
- **Denial of service** — unbounded loops or panics in library code on
  well-formed input. Every hot-path operation is documented as bounded per
  call; a reproducible violation of a documented bound is in scope.

**Already-documented deviations are not vulnerabilities in themselves.**
`SeqRing` carries a deliberate, documented formal data race (the seqlock
deviation — see the `seq_ring` module docs and `docs/records/seq-ring.md`),
and the counter-width span limits on `SeqRing` accounting,
`LatestBuf::skipped`, and `BlockBuilder` contiguity are documented
boundaries with stated reachability arithmetic. A report that one of these
*manifests beyond its documented bound* — or that the documentation
understates the exposure — is very much in scope and welcome.

## Disclosure

Once a fix is available, a security advisory will be published via
[GitHub Security Advisories](https://github.com/photon-circus/ph-eventing/security/advisories)
and the fix will be released as a patch version.
