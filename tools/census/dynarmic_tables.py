# SPDX-License-Identifier: GPL-3.0-only
"""Parse dynarmic A32 decoder tables and emulate its decode order.

Read-only: parses src/dynarmic/frontend/A32/decoder/*.inc and the translate
impl sources of a dynarmic checkout. Used by census.py / report.py.
"""
import os
import re
import numpy as np

INST_RE = re.compile(r'^\s*(//)?\s*INST\((\w+),\s*"([^"]*)",\s*"([01\-A-Za-z]+)"\)')


class M:
    __slots__ = ("fn", "name", "pat", "mask", "exp", "commented", "table")

    def __init__(self, fn, name, pat, commented, table):
        self.fn, self.name, self.pat, self.commented, self.table = fn, name, pat, commented, table
        n = len(pat)
        mask = exp = 0
        for i, ch in enumerate(pat):
            bit = n - 1 - i
            if ch in "01":
                mask |= 1 << bit
                exp |= (ch == "1") << bit
        self.mask, self.exp = mask, exp

    def popcount(self):
        return bin(self.mask).count("1")


def parse_inc(path, table):
    out = []
    for line in open(path):
        m = INST_RE.match(line)
        if m:
            out.append(M(m.group(2), m.group(3), m.group(4), bool(m.group(1)), table))
    return out


class Tables:
    def __init__(self, root):
        d = os.path.join(root, "src/dynarmic/frontend/A32/decoder")
        self.arm = parse_inc(os.path.join(d, "arm.inc"), "arm")
        self.vfp = parse_inc(os.path.join(d, "vfp.inc"), "vfp")
        self.asimd = parse_inc(os.path.join(d, "asimd.inc"), "asimd")
        self.t16 = parse_inc(os.path.join(d, "thumb16.inc"), "thumb16")
        self.t32 = parse_inc(os.path.join(d, "thumb32.inc"), "thumb32")
        act = lambda l: [m for m in l if not m.commented]
        self.commented = [m for l in (self.arm, self.vfp, self.asimd, self.t16, self.t32) for m in l if m.commented]
        # arm: stable sort by popcount desc
        self.arm_o = sorted(act(self.arm), key=lambda m: -m.popcount())
        # vfp: stable partition: unconditional (mask top nibble F) first
        v = act(self.vfp)
        self.vfp_u = [m for m in v if (m.mask & 0xF0000000) == 0xF0000000]
        self.vfp_c = [m for m in v if (m.mask & 0xF0000000) != 0xF0000000]
        # asimd ordering (see decoder/asimd.h)
        first = {"VBIC, VMOV, VMVN, VORR (immediate)", "VEXT", "VTBL", "VTBX", "VDUP (scalar)"}
        last = {"VMLA (scalar)", "VMLAL (scalar)", "VQDMLAL/VQDMLSL (scalar)", "VMUL (scalar)",
                "VMULL (scalar)", "VQDMULL (scalar)", "VQDMULH (scalar)", "VQRDMULH (scalar)"}
        a = act(self.asimd)
        f = [m for m in a if m.name in first]
        rest = [m for m in a if m.name not in first]
        mid = [m for m in rest if m.name not in last]
        l = [m for m in rest if m.name in last]
        mid.sort(key=lambda m: -m.popcount())
        self.asimd_o = f + mid + l
        self.t16_o = act(self.t16)
        self.t32_o = act(self.t32)
        self.by_fn = {m.fn: m for m in self.arm + self.vfp + self.asimd + self.t16 + self.t32}

    # ---- vectorised matching ------------------------------------------------
    @staticmethod
    def _stage(enc, res, ms, gate=None):
        for m in ms:
            hit = (res == None) if False else None  # placeholder to keep linter quiet
        return res

    def match_arm(self, enc):
        """enc: uint32 array of ARM words -> list of matcher-or-None per element, as object array."""
        return self._run(enc, [(self.vfp_u, lambda e: (e >> 28) == 0xF),
                               (self.vfp_c, lambda e: (e >> 28) != 0xF),
                               (self.asimd_o, None), (self.arm_o, None)])

    def match_t16(self, enc):
        return self._run(enc, [(self.t16_o, None)])

    def match_t32(self, enc):
        enc = enc.astype(np.uint32)
        maybe = ((enc & 0xEC000000) == 0xEC000000) | ((enc & 0xFF100000) == 0xF9000000)
        conv = np.full(enc.shape, 0xF7F0A000, dtype=np.uint32)
        c1 = (enc & 0xEF000000) == 0xEF000000
        u = (enc >> 28) & 1
        conv = np.where(c1, 0xF2000000 | (u << 24) | (enc & 0x00FFFFFF), conv)
        c2 = (enc & 0xFF000000) == 0xF9000000
        conv = np.where(c2 & ~c1, 0xF4000000 | (enc & 0x00FFFFFF), conv).astype(np.uint32)
        res = np.full(enc.shape, -1, dtype=np.int32)
        pool = []
        idx = {}

        def reg(m):
            if id(m) not in idx:
                idx[id(m)] = len(pool)
                pool.append(m)
            return idx[id(m)]
        # VFP, then ASIMD (on converted), then Thumb32 table
        for ms, gate in ((self.vfp_u, lambda e: (e >> 28) == 0xF), (self.vfp_c, lambda e: (e >> 28) != 0xF)):
            for m in ms:
                hit = maybe & (res < 0) & ((enc & m.mask) == m.exp) & gate(enc)
                if hit.any():
                    res[hit] = reg(m)
        for m in self.asimd_o:
            hit = maybe & (res < 0) & ((conv & m.mask) == m.exp)
            if hit.any():
                res[hit] = reg(m)
        for m in self.t32_o:
            hit = (res < 0) & ((enc & m.mask) == m.exp)
            if hit.any():
                res[hit] = reg(m)
        return res, pool

    def _run(self, enc, stages):
        enc = enc.astype(np.uint32)
        res = np.full(enc.shape, -1, dtype=np.int32)
        pool = []
        for ms, gate in stages:
            g = gate(enc) if gate else None
            for m in ms:
                hit = (res < 0) & ((enc & m.mask) == m.exp)
                if g is not None:
                    hit &= g
                if hit.any():
                    res[hit] = len(pool)
                    pool.append(m)
        return res, pool


# ---- handler body analysis ---------------------------------------------------
FN_RE = re.compile(r'bool\s+TranslatorVisitor::(\w+)\s*\(')
BAD = ("UndefinedInstruction", "InterpretThisInstruction", "UnpredictableInstruction")


def strip_comments(s):
    s = re.sub(r'/\*.*?\*/', '', s, flags=re.S)
    return re.sub(r'//[^\n]*', '', s)


def analyse_handlers(root):
    """fn -> (status, detail). status: OK | STUB | PARTIAL; detail: which bad call(s)."""
    d = os.path.join(root, "src/dynarmic/frontend/A32/translate/impl")
    out = {}
    for fn in sorted(os.listdir(d)):
        if not fn.endswith(".cpp"):
            continue
        src = strip_comments(open(os.path.join(d, fn)).read())
        for m in FN_RE.finditer(src):
            i = src.find("{", m.end())
            depth, j = 0, i
            while j < len(src):
                if src[j] == "{":
                    depth += 1
                elif src[j] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            body = src[i + 1:j]
            flat = re.sub(r'\s+', '', body)
            used = [b for b in BAD if b in body]
            if re.fullmatch(r'return(%s)\(\);' % "|".join(BAD), flat):
                out[m.group(1)] = ("STUB", used[0], fn)
            elif used:
                out[m.group(1)] = ("PARTIAL", ",".join(used), fn)
            else:
                out[m.group(1)] = ("OK", "", fn)
    return out
