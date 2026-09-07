"""Source filtering for the refusal inventory; test fixtures are not refusals."""
import re


def strip_test_modules(src):
    # Match braces in code, excluding strings/comments: fixture messages can
    # contain unmatched braces and must not terminate the test-module scan.
    code = re.sub(r'//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*"',
                  lambda m: " " * len(m.group()), src, flags=re.S)
    pattern = re.compile(
        r'#\[cfg\(\s*(?:all\(\s*)?test\b[^\]]*\]\s*'
        r'(?:#\[[^\]]*\]\s*)*mod\s+[A-Za-z_][A-Za-z_0-9]*\s*\{'
    )
    out, pos = [], 0
    while (match := pattern.search(code, pos)) is not None:
        start = match.end() - 1
        if start < 0:
            break
        out.append(src[pos:match.start()])
        depth, end = 1, start + 1
        while end < len(code) and depth:
            depth += (code[end] == "{") - (code[end] == "}")
            end += 1
        pos = end
    out.append(src[pos:])
    return "".join(out)

