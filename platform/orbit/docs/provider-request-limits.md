# Provider request-size limits

## Symptoms

A turn fails with one of:

```
Agent error: 413: {"message":"Request Entity Too Large (ref: …)","type":"api_error","param":null,"code":null}
Agent error: 400: {"message":"failed to read request body (ref: …)","type":"invalid_request_error","param":null,"code":null}
```

Both are the provider rejecting the HTTP request body, not an Orbit or pi bug. pi
composes the message in `pi-ai/dist/utils/error-body.js` as `<status>: <raw body>`.

## Cause

The request body exceeded the endpoint's upload cap. Images dominate: `read` on a
PNG stores it as base64 (≈4/3 the raw bytes) in the transcript, where it stays for
the rest of the session. A visual-debugging session that reads one screenshot per
turn crosses the cap after a dozen or so reads.

Measured against `https://ollama.com/v1/chat/completions` (Oct 2026):

| body size | result |
| --- | --- |
| ≤ 7.5 MB | 200 |
| ≥ 8.08 MB | `413 Request Entity Too Large` |
| tens of MB | connection cut mid-upload → `400 failed to read request body` |

Image *count* is not the limit (64 tiny images in an 11 KB body succeed). One
failing session held 35 images ≈ 21 MB raw ≈ 29 MB base64.

Note that `~/.pi/agent/settings.json` `httpIdleTimeoutMs` is unrelated: a request
that stalls for that long fails with `Request timed out.`, not a 413/400.

## Fix

**Orbit compresses images at attach time.**
images you attach or paste are downscaled to a 1400 px longest edge and
re-encoded — opaque images as JPEG (quality steps down to keep the base64
payload within 512 KiB), transparent images as lossless WebP — before they
ever reach pi. Images already under both caps pass through untouched, and
animated GIFs pass through so their frames survive. This is the first line of
defense and usually keeps the request body under the cap on its own; the
`models.json` override below remains useful as a provider-side backstop and
for images pi itself reads with its tools.

1. **Unblock the session.** Retrying, `continue`, or re-sending cannot succeed
   while the oversized images are still in history. Start a new session, or
   `/compact` and confirm the recent window no longer holds the screenshots.
2. **Make pi downscale new images.** Declare resize limits for the model in
   `~/.pi/agent/models.json` (Orbit only edits the `providers` object, so a
   hand-written file survives a round trip):

   ```json
   {
     "providers": {
       "ollama-cloud": {
         "modelOverrides": {
           "deepseek-v4.1-flash": {
             "inputLimits": {
               "images": {
                 "resize": {
                   "maxWidth": 1400,
                   "maxHeight": 1400,
                   "maxBytes": 524288,
                   "jpegQuality": 75
                 }
               }
             }
           }
         }
       }
     }
   }
   ```

   `modelOverrides` changes an existing built-in or extension model without
   replacing the provider's catalog; unknown model IDs are ignored. `maxBytes`
   caps the **base64** payload per image. This affects images stored from now on:
   pi does not rewrite or reject images already in history
   (`inputLimits.maxRequestBytes`, `images.maxPerMessage`, and
   `images.maxPerRequest` are descriptive only). Adding the override for every
   vision model on the provider avoids losing it on the next model switch.
3. **Capture smaller screenshots.** JPEG at ~70% quality is roughly 10× smaller
   than a UI PNG for the same content.
4. **Keep sessions short.** The per-image cap buys headroom, not immunity: with
   512 KB images, ~15 images still approach an 8 MB cap. Start a fresh session
   per visual-debugging pass instead of accumulating screenshots all day.

## Upstream fix (`pi-ollama-cloud`)

The extension's catalog was the only one missing resize limits: `opencode-go`,
`together`, `deepseek`, and `baseten` all declare
`inputLimits.images.resize`, but `pi-ollama-cloud` emitted `input` without
`inputLimits`, so pi sent full-size images.

`contrib/pi-ollama-cloud-input-limits.patch` fixes the extension:

- `models.ts` exports `VISION_INPUT_LIMITS` (1400×1400, 512 KiB base64, JPEG 75)
  and emits it from `assembleModels` for models with the `vision` capability.
- `models.generated.ts` carries the same limits on its eight vision models.
- `test/models.test.ts` asserts vision models get the limits, text-only models do
  not, and the baked-in catalog matches `assembleModels`.
- `README.md` / `CHANGELOG.md` document the behaviour.

Apply it against the upstream repository:

```sh
git clone https://github.com/fgrehm/pi-ollama-cloud.git
cd pi-ollama-cloud
git apply /path/to/orbit/contrib/pi-ollama-cloud-input-limits.patch
npm ci && npm run lint && npm run typecheck && npm test
```

Verified against upstream HEAD (`adea8d9`): the patch applies cleanly, lint and
typecheck pass, and all 178 tests pass. Until a release ships this, keep the
`models.json` override — `pi update` reinstalls the extension from npm and drops
any local edit.

## Verifying the override

After saving `models.json`, `read` an image in a new session and inspect the
stored block: the tool-result text reports the new `displayed at` dimensions, and
`data` in the session JSONL should fit under `maxBytes`.

```sh
# in a fresh session, read an image, then:
F=$(ls -t ~/.pi/agent/sessions/*/*.jsonl | head -1)
python3 - "$F" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    content = (json.loads(line).get("message") or {}).get("content") or []
    for block in content:
        if not isinstance(block, dict):
            continue
        for part in block.get("content") or []:
            if isinstance(part, dict) and part.get("type") == "image":
                print("base64 chars:", len(part["data"]))
PY
```
