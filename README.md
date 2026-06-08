# Prime archive tools

## Local UI

Build and start the local UI:

```bash
make ui-start ARCHIVE_DIR=data/runs/count_1e11/archive
```

Open:

```text
http://127.0.0.1:43173
```

Useful commands:

```bash
make ui-status
make ui-stop
```

To use a different archive:

```bash
make ui-start ARCHIVE_DIR=/path/to/archive UI_PORT=43174
```
