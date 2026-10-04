"""Independent semantic-input CBOR profile consumer. Representation only, no auth."""
import argparse
import hashlib
import json
from pathlib import Path


def integer(value, signed=False):
    n = int(value)
    if str(n) != value or not (-(1 << 63) if signed else 0) <= n <= ((1 << 63) - 1 if signed else (1 << 64) - 1):
        raise ValueError("integer range or spelling")
    return n


def head(major, n):
    if n < 24:
        return bytes([(major << 5) | n])
    for width, additional in ((1, 24), (2, 25), (4, 26), (8, 27)):
        if n < 1 << (8 * width):
            return bytes([(major << 5) | additional]) + n.to_bytes(width, "big")
    raise ValueError("overflow")


def cbor(value):
    if value is None:
        return b"\xf6"
    if isinstance(value, bool):
        return b"\xf5" if value else b"\xf4"
    if isinstance(value, int):
        return head(0, value) if value >= 0 else head(1, -1 - value)
    if isinstance(value, bytes):
        return head(2, len(value)) + value
    if isinstance(value, str):
        raw = value.encode("utf-8")
        return head(3, len(raw)) + raw
    if isinstance(value, list):
        return head(4, len(value)) + b"".join(cbor(v) for v in value)
    if isinstance(value, dict):
        return head(5, len(value)) + b"".join(cbor(k) + cbor(value[k]) for k in sorted(value))
    raise ValueError("type")


def semantic(ty, value, types):
    if ty == "String":
        if not isinstance(value, str):
            raise ValueError("text")
        return value
    if ty in ("Digest", "Bytes"):
        raw = bytes.fromhex(value)
        if ty == "Digest" and len(raw) != 32:
            raise ValueError("digest")
        return raw
    if ty in ("u64", "usize"):
        return integer(value).to_bytes(8, "big")
    if ty in ("i64", "u8"):
        result = integer(value, ty == "i64")
        if ty == "u8" and result > 255:
            raise ValueError("byte")
        return result
    if ty == "bool":
        if not isinstance(value, bool):
            raise ValueError("bool")
        return value
    if ty.startswith("Vec<"):
        return [semantic(ty[4:-1], v, types) for v in value]
    if ty.startswith("Option<"):
        return None if value is None else [semantic(ty[7:-1], value, types)]
    if ty.startswith("Box<"):
        return semantic(ty[4:-1], value, types)
    if ty == "Operation":
        op = {1: value["share"], 2: value["glade_id"], 3: bytes.fromhex(value["key"]), 4: value["origin"], 5: integer(value["seq"], True), 6: None if value["prev"] is None else bytes.fromhex(value["prev"]), 7: integer(value["lamport"], True), 8: [{1: h["origin"], 2: integer(h["seq"], True), 3: bytes.fromhex(h["hash"])} for h in value["refs"]], 9: integer(value["shape"], True), 10: bytes.fromhex(value["payload"])}
        return cbor(op)
    schema = types[ty]
    if schema["kind"] == "enum":
        for index, (variant, fields) in enumerate(schema["variants"]):
            if variant == value["variant"]:
                if set(value["fields"]) != {name for name, _ in fields}:
                    raise ValueError("enum fields")
                return [index] + [semantic(t, value["fields"][name], types) for name, t in fields]
        raise ValueError("enum")
    fields = schema["fields"]
    if set(value) != {name for name, _ in fields}:
        raise ValueError("unknown or missing fields")
    return {index: semantic(t, value[name], types) for index, (name, t) in enumerate(fields)}


def profile(kind, value, types):
    body = semantic(kind, value, types)
    return cbor({0: 1, 1: kind, **{k + 2: v for k, v in body.items()}})


def decode(raw, limit=262144, items=8192, depth=64):
    if len(raw) > limit:
        raise ValueError("limit")
    index = 0
    remaining = items

    def take(n):
        nonlocal index
        if n > len(raw) - index:
            raise ValueError("truncated")
        result = raw[index:index + n]
        index += n
        return result

    def value(level):
        nonlocal remaining
        remaining -= 1
        if remaining < 0 or level > depth:
            raise ValueError("budget")
        byte = take(1)[0]
        major, additional = byte >> 5, byte & 31
        if additional < 24:
            n = additional
        elif additional in (24, 25, 26, 27):
            width = 1 << (additional - 24)
            n = int.from_bytes(take(width), "big")
            if n < (24 if width == 1 else 1 << (8 * (width // 2))):
                raise ValueError("nonshortest")
        else:
            raise ValueError("indefinite")
        if major == 0:
            if n > (1 << 63) - 1:
                raise ValueError("wire int")
            return n
        if major == 1:
            if n > (1 << 63) - 1:
                raise ValueError("wire int")
            return -1 - n
        if major == 2:
            return take(n)
        if major == 3:
            if n > 256:
                raise ValueError("name")
            return take(n).decode("utf-8", "strict")
        if major == 4:
            if n > remaining:
                raise ValueError("items")
            return [value(level + 1) for _ in range(n)]
        if major == 5:
            if n > remaining // 2:
                raise ValueError("items")
            result = {}
            previous = -1
            for _ in range(n):
                key = value(level + 1)
                if not isinstance(key, int) or isinstance(key, bool) or key <= previous:
                    raise ValueError("duplicate/order")
                previous = key
                result[key] = value(level + 1)
            return result
        if major == 7 and additional in (20, 21, 22):
            return (False, True, None)[additional - 20]
        raise ValueError("unsupported")

    result = value(0)
    if index != len(raw):
        raise ValueError("trailing")
    return result


def validate_profile(raw, kind, types):
    value = decode(raw)
    count = len(types[kind]["fields"]) + 2
    if not isinstance(value, dict) or list(value) != list(range(count)) or value[0] != 1 or value[1] != kind:
        raise ValueError("closed versioned profile")
    if kind == "SignedRecord":
        domains = {"glade/ic3/v1/" + name + "\0" for name in ("descriptor", "origin-certificate", "offline-permit", "policy-cut", "application-op", "local-session", "admission", "inventory")}
        if value[2] not in domains or value[3] != (1).to_bytes(8, "big") or len(value[4]) != 32 or len(value[6]) != 64:
            raise ValueError("signed envelope shape")
    return value

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--produce", action="store_true")
    args = parser.parse_args()
    corpus = json.loads((args.root / "vectors/input.json").read_text())
    types = json.loads((args.root / "vectors/schema.json").read_text())["types"]
    results = {}
    for case in corpus["cases"]:
        raw = profile(case["kind"], case["value"], types)
        assert cbor(validate_profile(raw,case["kind"],types)) == raw
        results[case["id"]] = {"hex": raw.hex(), "sha256": hashlib.sha256(raw).hexdigest()}
    if args.produce:
        (args.root / "vectors/expected.json").write_text(json.dumps(results, indent=2) + "\n")
    else:
        assert results == json.loads((args.root / "vectors/expected.json").read_text())
        for negative in json.loads((args.root / "vectors/negative.json").read_text())["cases"]:
            try:
                validate_profile(bytes.fromhex(negative["hex"]), negative["kind"], types) if "kind" in negative else decode(bytes.fromhex(negative["hex"]), **negative.get("limits", {}))
            except (ValueError, UnicodeDecodeError):
                continue
            raise AssertionError(negative["id"])
        print(f"Python independent profile vectors: {len(results)} positive + negative controls PASS")


if __name__ == "__main__":
    main()
