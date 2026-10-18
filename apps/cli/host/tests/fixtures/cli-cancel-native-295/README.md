# Native 2.1.295 CLI cancellation fixture

These three raw files are byte-for-byte copies from Harness commit `b3864c6ca6c16ed682a333ad96f7ad59cd05fb0b`, under `harness-runtime/docs/parity/current-alignment/fixtures/cli-cancel-295/native-baseline/2.1.295/`. The source `harness-runtime/docs/parity/current-alignment/fixtures/cli-cancel-295/native-baseline/2.1.295/native-receipt.json` SHA-256 is `43b194650c95c91dcf61458300e4ecf428f5bc4e4ac137e038258c0ff8ed8e2d`; it binds the capture to Native 2.1.295 binary SHA-256 `0116ee2e0a513900b633d9951367f18747686478e2b462805b8c31609f047f70`.

| File | Source SHA-256 |
| --- | --- |
| `stdout.raw` | `589d30e369a75200eef22c16e72fc068716736f988352f1748991f9a559525c2` |
| `prompt-content.raw.json` | `63738af050e51a7c05269ea26e17169ada3ac1ba1d41fec1ff88be3a8fb9647b` |
| `interrupt-content.raw.json` | `ad710d55159f0013958b33520d37ce6d14c6f3647d93c09d005c7e545bebe228` |

The captured public `--print --output-format json` process received SIGINT after its single streaming localhost request was accepted; the provider sent no response. The CLI test compares stable terminal fields and the two raw session `message.content` tokens. Dynamic stdout and transcript envelopes are not byte-equality claims.
