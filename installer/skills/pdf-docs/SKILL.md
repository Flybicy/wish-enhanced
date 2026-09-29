---
name: pdf-docs
description: 用捆绑的 Python 处理 PDF：合并、拆分、提取文本、生成新 PDF
triggers: pdf, 报销, 打印, 合并, 拆分, 提取
---
# PDF 处理（pypdf + reportlab）

> Python 调用约定：先 ``python -c "import docx"`` 试探；报错说明系统 python 缺文档库，改用 Wish 自带的 ``D:\Wish\python\python.exe``（文档库只装在自带环境）。
Wish 自带 Python（PATH 上直接 python；不在则 D:\Wish\python\python.exe）。

## 合并
```python
from pypdf import PdfWriter
w = PdfWriter()
for f in ["a.pdf", "b.pdf"]:
    w.append(f)
w.write("merged.pdf")
```

## 拆分与提取
```python
from pypdf import PdfReader
r = PdfReader("in.pdf")
print(len(r.pages))                       # 页数
w = PdfWriter(); w.add_page(r.pages[0]); w.write("first.pdf")   # 拆第一页
text = "\n".join(p.extract_text() or "" for p in r.pages)       # 提取文本
```

## 生成新 PDF（reportlab，支持中文字体）
```python
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.pdfgen import canvas
pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light"))
c = canvas.Canvas("out.pdf", pagesize=A4)
c.setFont("STSong-Light", 14)
c.drawString(72, 800, "中文内容 OK")
c.save()
```

## 习惯
1. 中文用 STSong-Light（CID 内置，无需字体文件）。
2. 处理前打印页数，处理后核对输出页数。