# shipit

Uploads build artifacts, retrying failed uploads.

## Settings

| setting       | default | meaning                                      |
|---------------|---------|----------------------------------------------|
| `retry_limit` | 3       | how many times to try an upload before failing |
| `timeout_s`   | 30      | seconds to wait for one upload               |

Set them in `shipit.toml`, or on the command line:

```sh
python3 -m shipit.cli --retry-limit 5 --timeout-s 10 dist/app.tar.gz
```

`retry_limit` must be at least 1.
