#!/usr/bin/env python3
"""
Helper script for the knr-ssa skill:
Generates valid, strictly monotonic SSA instructions for VDOM components
(struct[Leaf], struct[Card], struct[Section]) from a JSON specification,
eliminating manual register numbering and UseAfterMove errors.

Usage:
    python3 gen_vdom_ssa.py spec.json > component.ssa
"""
import json
import sys


class SsaBuilder:
    def __init__(self, start_reg: int = 0):
        self.reg = start_reg
        self.lines = []

    def next_reg(self) -> int:
        r = self.reg
        self.reg += 1
        return r

    def emit(self, expr: str) -> int:
        r = self.next_reg()
        self.lines.append(f"    %{r} = {expr}")
        return r

    def const_str(self, s: str) -> int:
        escaped = json.dumps(s, ensure_ascii=False)
        return self.emit(f"const_str {escaped}")

    def build_leaf(self, leaf: dict) -> int:
        cur = self.emit("struct_new Leaf")
        if "tag" in leaf and leaf["tag"] is not None:
            s = self.const_str(leaf["tag"])
            cur = self.emit(f"struct_set %{cur} 0 %{s}")
        if "class" in leaf and leaf["class"] is not None:
            s = self.const_str(leaf["class"])
            cur = self.emit(f"struct_set %{cur} 1 %{s}")
        if "text" in leaf and leaf["text"] is not None:
            s = self.const_str(leaf["text"])
            cur = self.emit(f"struct_set %{cur} 2 %{s}")
        if "action" in leaf and leaf["action"] is not None:
            s = self.const_str(leaf["action"])
            cur = self.emit(f"struct_set %{cur} 3 %{s}")
        if "target" in leaf and leaf["target"] is not None:
            s = self.const_str(leaf["target"])
            cur = self.emit(f"struct_set %{cur} 4 %{s}")
        return cur

    def build_card(self, card: dict) -> int:
        vec = self.emit("vec_new struct[Leaf]")
        for leaf in card.get("children", []):
            leaf_reg = self.build_leaf(leaf)
            vec = self.emit(f"vec_push %{vec} %{leaf_reg}")
        cur = self.emit("struct_new Card")
        if "tag" in card and card["tag"] is not None:
            s = self.const_str(card["tag"])
            cur = self.emit(f"struct_set %{cur} 0 %{s}")
        if "class" in card and card["class"] is not None:
            s = self.const_str(card["class"])
            cur = self.emit(f"struct_set %{cur} 1 %{s}")
        if "text" in card and card["text"] is not None:
            s = self.const_str(card["text"])
            cur = self.emit(f"struct_set %{cur} 2 %{s}")
        cur = self.emit(f"struct_set %{cur} 3 %{vec}")
        return cur

    def build_section(self, sec: dict) -> int:
        vec = self.emit("vec_new struct[Card]")
        for card in sec.get("children", []):
            card_reg = self.build_card(card)
            vec = self.emit(f"vec_push %{vec} %{card_reg}")
        cur = self.emit("struct_new Section")
        if "tag" in sec and sec["tag"] is not None:
            s = self.const_str(sec["tag"])
            cur = self.emit(f"struct_set %{cur} 0 %{s}")
        if "class" in sec and sec["class"] is not None:
            s = self.const_str(sec["class"])
            cur = self.emit(f"struct_set %{cur} 1 %{s}")
        if "text" in sec and sec["text"] is not None:
            s = self.const_str(sec["text"])
            cur = self.emit(f"struct_set %{cur} 2 %{s}")
        cur = self.emit(f"struct_set %{cur} 3 %{vec}")
        return cur


def main():
    if len(sys.argv) < 2:
        print(__doc__.strip(), file=sys.stderr)
        sys.exit(1)
    with open(sys.argv[1], "r", encoding="utf-8") as f:
        spec = json.load(f)

    imports = spec.get("imports", ["../schemas/vdom.ssa.schema"])
    fn_name = spec["fn_name"]
    params = spec.get("params", [])
    ret_type = spec.get("ret_type", "struct[Section]")
    builder = SsaBuilder(start_reg=len(params))

    if ret_type == "struct[Section]":
        ret_reg = builder.build_section(spec["root"])
    elif ret_type == "struct[Card]":
        ret_reg = builder.build_card(spec["root"])
    else:
        ret_reg = builder.build_leaf(spec["root"])

    for imp in imports:
        print(f'import "{imp}"')
    print()
    param_str = ", ".join(f"%{i}: {t}" for i, t in enumerate(params))
    print(f"fn {fn_name}({param_str}) -> {ret_type} {{")
    print("bb0:")
    for line in builder.lines:
        print(line)
    print(f"    ret %{ret_reg}")
    print("}")


if __name__ == "__main__":
    main()
