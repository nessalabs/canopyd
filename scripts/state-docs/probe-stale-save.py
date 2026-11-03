#!/usr/bin/env python3
"""Exercise actual Registry APIs using an isolated temp crate and registry; no real bind/ports."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix="canopyd-stale-save-") as scratch:
    temp = Path(scratch)
    (temp / "src").mkdir()
    (temp / "src/main.rs").write_text((root / "verification/source-probes/stale-save.rs").read_text())
    (temp / "Cargo.toml").write_text('[package]\nname="canopyd-stale-save-probe"\nversion="0.0.0"\nedition="2024"\n[dependencies]\ncanopyd={path=' + json.dumps(str(root)) + '}\ncamino="1.2"\n')
    env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target/state-docs-probe")}
    result = subprocess.run(["cargo", "run", "--quiet", "--manifest-path", str(temp / "Cargo.toml"), "--", str(temp / "ports.json")], env=env)
    raise SystemExit(result.returncode)
