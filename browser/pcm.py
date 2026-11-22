"""PCM wave framing with a declared sample count; no resampling or gain."""

import struct


def header(format):
    bits = format["bits"]
    channels = format["channels"]
    rate = format["sample_rate"]
    frames = format["frames"]
    if (
        bits not in (16, 24)
        or channels not in (1, 2)
        or not 8000 <= rate <= 384000
        or not frames > 0
    ):
        raise ValueError("Invalid PCM stream format")
    width = bits // 8
    length = frames * channels * width
    if length > 0xFFFFFFFF - 36:
        raise ValueError("Track exceeds the WAVE size limit")
    return (
        b"RIFF"
        + struct.pack("<I", length + 36)
        + b"WAVEfmt "
        + struct.pack(
            "<IHHIIHH",
            16,
            1,
            channels,
            rate,
            rate * channels * width,
            channels * width,
            bits,
        )
        + b"data"
        + struct.pack("<I", length),
        length,
    )
