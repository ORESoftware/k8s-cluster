#!/usr/bin/env python3
"""Create a deterministic USTAR archive without dereferencing symlinks."""

from __future__ import annotations

import os
from pathlib import Path
import stat
import sys
import tarfile


def iter_paths(root: Path):
    yield Path(".")
    for current, directories, files in os.walk(root, topdown=True, followlinks=False):
        directories.sort()
        files.sort()
        current_path = Path(current)
        for name in [*directories, *files]:
            yield (current_path / name).relative_to(root)


def normalized_mode(file_stat: os.stat_result) -> int:
    if stat.S_ISDIR(file_stat.st_mode):
        return 0o755
    if stat.S_ISREG(file_stat.st_mode):
        return 0o755 if file_stat.st_mode & stat.S_IXUSR else 0o644
    if stat.S_ISLNK(file_stat.st_mode):
        return 0o777
    raise ValueError("artifact contains an unsupported special file")


def main() -> None:
    if len(sys.argv) != 4:
        raise SystemExit("usage: create-deterministic-tar.py ROOT OUTPUT SOURCE_DATE_EPOCH")

    root = Path(sys.argv[1]).resolve(strict=True)
    output = Path(sys.argv[2]).resolve()
    epoch = int(sys.argv[3])
    try:
        output.relative_to(root)
    except ValueError:
        pass
    else:
        raise ValueError("archive output must be outside the deployed tree")

    with tarfile.open(output, "w", format=tarfile.USTAR_FORMAT, dereference=False) as archive:
        for relative in iter_paths(root):
            absolute = root if relative == Path(".") else root / relative
            before = os.lstat(absolute)
            archive_name = "." if relative == Path(".") else f"./{relative.as_posix()}"
            info = archive.gettarinfo(str(absolute), arcname=archive_name)
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = epoch
            info.mode = normalized_mode(before)

            if stat.S_ISREG(before.st_mode):
                descriptor = os.open(absolute, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
                with os.fdopen(descriptor, "rb") as source:
                    after = os.fstat(source.fileno())
                    if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
                        raise RuntimeError(f"artifact path changed during packaging: {relative}")
                    archive.addfile(info, source)
            else:
                archive.addfile(info)


if __name__ == "__main__":
    main()
