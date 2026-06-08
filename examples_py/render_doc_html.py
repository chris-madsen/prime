from __future__ import annotations

import html
import re
import sys
from pathlib import Path


def render_inline(text: str) -> str:
    def render_inline_no_links(chunk: str) -> str:
        parts = re.split(r'(`[^`]+`)', chunk)
        rendered: list[str] = []
        for part in parts:
            if part.startswith('`') and part.endswith('`') and len(part) >= 2:
                rendered.append(f"<code>{html.escape(part[1:-1])}</code>")
                continue
            escaped = html.escape(part)
            escaped = re.sub(r'\*\*([^*]+)\*\*', r'<strong>\1</strong>', escaped)
            rendered.append(escaped)
        return ''.join(rendered)

    result: list[str] = []
    pos = 0
    for match in re.finditer(r'\[([^\]]+)\]\(([^)]+)\)', text):
        result.append(render_inline_no_links(text[pos:match.start()]))
        label = render_inline_no_links(match.group(1))
        href = html.escape(match.group(2), quote=True)
        result.append(f'<a href="{href}">{label}</a>')
        pos = match.end()
    result.append(render_inline_no_links(text[pos:]))
    return ''.join(result)


def render_nested_bullets(block_lines: list[str]) -> str:
    html_parts: list[str] = []
    stack: list[int] = []

    for raw in block_lines:
        indent = len(raw) - len(raw.lstrip(' '))
        stripped = raw.strip()
        item = re.sub(r'^-\s+', '', stripped)

        while stack and indent < stack[-1]:
            html_parts.append('</li></ul>')
            stack.pop()

        if not stack or indent > stack[-1]:
            html_parts.append('<ul>')
            stack.append(indent)
        else:
            html_parts.append('</li>')

        html_parts.append(f'<li>{render_inline(item)}')

    while stack:
        html_parts.append('</li></ul>')
        stack.pop()

    return ''.join(html_parts)


def render_markdown(md: str) -> str:
    lines = md.splitlines()
    out: list[str] = []
    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        if not stripped:
            i += 1
            continue

        if stripped.startswith('<a ') and stripped.endswith('></a>'):
            out.append(stripped)
            i += 1
            continue

        if stripped.startswith('```'):
            lang = stripped[3:].strip()
            code_lines: list[str] = []
            i += 1
            while i < len(lines) and not lines[i].strip().startswith('```'):
                code_lines.append(lines[i])
                i += 1
            if i < len(lines):
                i += 1
            cls = f' class="language-{html.escape(lang)}"' if lang else ''
            out.append(f'<pre><code{cls}>{html.escape(chr(10).join(code_lines))}</code></pre>')
            continue

        if stripped == '$$':
            math_lines: list[str] = []
            i += 1
            while i < len(lines) and lines[i].strip() != '$$':
                math_lines.append(lines[i])
                i += 1
            if i < len(lines):
                i += 1
            formula = '\n'.join(math_lines)
            out.append(f'<div class="math-block">$$\n{html.escape(formula)}\n$$</div>')
            continue

        if re.fullmatch(r'-{3,}', stripped):
            out.append('<hr />')
            i += 1
            continue

        m = re.match(r'^(#{1,6})\s+(.*)$', line)
        if m:
            level = len(m.group(1))
            raw_heading = m.group(2).strip()
            content = render_inline(raw_heading)
            out.append(f'<h{level}>{content}</h{level}>')
            i += 1
            if level == 2 and raw_heading == 'Оглавление':
                j = i
                while j < len(lines) and not lines[j].strip():
                    j += 1
                if j < len(lines) and re.match(r'^\s*-\s+', lines[j].strip()):
                    i = j
                    out.append('<nav class="toc">')
                    toc_lines: list[str] = []
                    while i < len(lines) and re.match(r'^\s*-\s+', lines[i].strip()):
                        toc_lines.append(lines[i])
                        i += 1
                    out.append(render_nested_bullets(toc_lines))
                    out.append('</nav>')
                continue
            continue

        if stripped.startswith('|'):
            table_lines = []
            while i < len(lines) and lines[i].strip().startswith('|'):
                table_lines.append(lines[i].strip())
                i += 1
            rows = [[cell.strip() for cell in row.strip('|').split('|')] for row in table_lines]
            header = rows[0]
            out.append('<table>')
            out.append('<thead><tr>' + ''.join(f'<th>{render_inline(cell)}</th>' for cell in header) + '</tr></thead>')
            if len(rows) > 2:
                out.append('<tbody>')
                for row in rows[2:]:
                    out.append('<tr>' + ''.join(f'<td>{render_inline(cell)}</td>' for cell in row) + '</tr>')
                out.append('</tbody>')
            out.append('</table>')
            continue

        if stripped.startswith('> '):
            quote_lines = []
            while i < len(lines) and lines[i].strip().startswith('> '):
                quote_lines.append(lines[i].strip()[2:])
                i += 1
            out.append(f'<blockquote><p>{render_inline(" ".join(quote_lines))}</p></blockquote>')
            continue

        if re.match(r'^-\s+', stripped):
            list_lines: list[str] = []
            while i < len(lines) and re.match(r'^\s*-\s+', lines[i].strip()):
                list_lines.append(lines[i])
                i += 1
            out.append(render_nested_bullets(list_lines))
            continue

        if re.match(r'^\d+\.\s+', stripped):
            out.append('<ol>')
            while i < len(lines) and re.match(r'^\d+\.\s+', lines[i].strip()):
                item = re.sub(r'^\d+\.\s+', '', lines[i].strip())
                out.append(f'<li>{render_inline(item)}</li>')
                i += 1
            out.append('</ol>')
            continue

        para_lines = [stripped]
        i += 1
        while i < len(lines):
            nxt = lines[i].strip()
            if not nxt:
                break
            if nxt.startswith('<a ') and nxt.endswith('></a>'):
                break
            if nxt.startswith('```') or nxt.startswith('|') or re.fullmatch(r'-{3,}', nxt) or re.match(r'^(#{1,6})\s+', nxt) or re.match(r'^-\s+', nxt) or re.match(r'^\d+\.\s+', nxt):
                break
            para_lines.append(nxt)
            i += 1
        out.append(f'<p>{render_inline(" ".join(para_lines))}</p>')
    return '\n'.join(out)


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit('usage: render_doc_html.py input.md output.html')
    src = Path(sys.argv[1])
    dst = Path(sys.argv[2])
    md = src.read_text()
    body = render_markdown(md)
    title = 'Document'
    for line in md.splitlines():
        if line.startswith('# '):
            title = line[2:].strip().replace('`', '')
            break
    html_doc = f'''<!doctype html>
<html lang="ru">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{html.escape(title)}</title>
  <script>
    window.MathJax = {{
      tex: {{
        inlineMath: [['$', '$'], ['\\\\(', '\\\\)']],
        displayMath: [['$$', '$$'], ['\\\\[', '\\\\]']]
      }},
      options: {{
        skipHtmlTags: ['script', 'noscript', 'style', 'textarea', 'pre', 'code']
      }}
    }};
  </script>
  <script defer src="https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-chtml.js"></script>
  <style>
    :root {{
      --bg: #f5f3ef;
      --paper: #fffdfa;
      --text: #222222;
      --muted: #5a5a5a;
      --line: #ddd4c7;
      --soft: #f0ebe3;
      --link: #1f5fbf;
      --code-bg: #f4efe7;
      --quote-bg: #f8f4ee;
    }}
    * {{ box-sizing: border-box; }}
    html {{ scroll-behavior: smooth; }}
    body {{
      margin: 0;
      background: var(--bg);
      color: var(--text);
      font-family: "Iowan Old Style", "Palatino Linotype", "Book Antiqua", Georgia, serif;
      line-height: 1.75;
      font-size: 18px;
    }}
    main {{
      max-width: 920px;
      margin: 40px auto;
      background: var(--paper);
      padding: 48px 56px 64px;
      border: 1px solid var(--line);
      border-radius: 18px;
      box-shadow: 0 18px 40px rgba(30, 25, 20, 0.08);
    }}
    h1, h2, h3, h4 {{
      font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
      line-height: 1.22;
      letter-spacing: -0.01em;
      color: #171717;
      margin-top: 1.9em;
      margin-bottom: 0.7em;
    }}
    h1 {{
      font-size: 2.35rem;
      margin-top: 0;
      margin-bottom: 1.1rem;
    }}
    h2 {{
      font-size: 1.65rem;
      padding-bottom: 0.28rem;
      border-bottom: 1px solid var(--line);
    }}
    h3 {{
      font-size: 1.18rem;
    }}
    p {{ margin: 0.85rem 0; }}
    a {{
      color: var(--link);
      text-decoration: none;
    }}
    a:hover {{ text-decoration: underline; }}
    code {{
      font-family: ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
      background: var(--code-bg);
      padding: 0.12rem 0.32rem;
      border-radius: 6px;
      font-size: 0.92em;
    }}
    pre {{
      overflow-x: auto;
      padding: 1rem 1.1rem;
      border-radius: 12px;
      background: var(--code-bg);
      border: 1px solid var(--line);
      line-height: 1.55;
    }}
    pre code {{
      background: transparent;
      padding: 0;
      border-radius: 0;
    }}
    hr {{
      border: 0;
      border-top: 1px solid var(--line);
      margin: 2.2rem 0;
    }}
    ul, ol {{
      padding-left: 1.4rem;
      margin: 0.7rem 0 1rem;
    }}
    li {{
      margin: 0.22rem 0;
    }}
    li > ul {{
      margin-top: 0.35rem;
    }}
    table {{
      border-collapse: collapse;
      width: 100%;
      margin: 1.2rem 0 1.6rem;
      font-size: 0.96rem;
    }}
    th, td {{
      border: 1px solid var(--line);
      padding: 0.58rem 0.72rem;
      text-align: left;
      vertical-align: top;
    }}
    th {{
      background: var(--soft);
      font-family: Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
      font-weight: 600;
    }}
    blockquote {{
      margin: 1.2rem 0;
      padding: 0.8rem 1rem;
      border-left: 4px solid #cfb998;
      background: var(--quote-bg);
      color: var(--muted);
      border-radius: 10px;
    }}
    blockquote p {{ margin: 0; }}
    .toc {{
      margin: 1.4rem 0 2rem;
      padding: 1.15rem 1.25rem;
      border: 1px solid var(--line);
      border-radius: 14px;
      background: linear-gradient(180deg, #fcfaf6 0%, #f6f0e8 100%);
    }}
    .toc ul {{
      list-style: none;
      padding-left: 0;
      margin: 0;
    }}
    .toc li {{
      margin: 0.34rem 0;
    }}
    .toc li ul {{
      margin-top: 0.35rem;
      padding-left: 1rem;
      border-left: 2px solid rgba(207, 185, 152, 0.55);
    }}
    .math-block {{
      overflow-x: auto;
      margin: 1rem 0 1.2rem;
    }}
    .MathJax {{
      font-size: 1.02em !important;
    }}
    @media (max-width: 900px) {{
      body {{
        font-size: 17px;
      }}
      main {{
        margin: 0;
        border-radius: 0;
        border-left: 0;
        border-right: 0;
        padding: 28px 20px 42px;
      }}
      h1 {{
        font-size: 2rem;
      }}
      h2 {{
        font-size: 1.45rem;
      }}
    }}
  </style>
</head>
<body>
  <main>
{body}
  </main>
</body>
</html>
'''
    dst.write_text(html_doc)


if __name__ == '__main__':
    main()
