# tests/fixtures/

Binary test fixtures for integration tests.

## Contents (added as needed)

- `sample.h264` — A short H.264 NAL unit sequence for decoder tests (added in P1.5)
- `sample.opus` — A short Opus packet sequence for audio decoder tests (added in Phase 3)

## Guidelines

- Keep fixtures small (< 100KB each).
- Document the source and generation command for each fixture.
- Do not commit generated build artifacts here — only hand-crafted or tool-generated test inputs.
