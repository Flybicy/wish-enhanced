---
name: office-docs
description: 用捆绑的 Python（python-docx）创建和编辑 Word 文档
triggers: word, docx, 文档, 报告, 合同, 简历, 标书
---
# Word 文档（python-docx）

> Python 调用约定：先 ``python -c "import docx"`` 试探；报错说明系统 python 缺文档库，改用 Wish 自带的 ``D:\Wish\python\python.exe``（文档库只装在自带环境）。
Wish 自带 Python（wish 安装目录下 python\python.exe，一般已在 PATH 上，直接用 python 即可；若不在 PATH，找 D:\Wish\python\python.exe）。

## 快速上手
```python
import docx
doc = docx.Document()
doc.add_heading("标题", 0)
doc.add_paragraph("正文段落。")
doc.add_heading("一级标题", 1)
doc.add_paragraph("带样式：", style="Intense Quote")
doc.add_page_break()
table = doc.add_table(rows=2, cols=3)
table.style = "Light Grid Accent 1"
table.rows[0].cells[0].text = "列头"
doc.save("out.docx")
```

## 常用手法
- 加粗行内文字：run = p.add_run("重点"); run.bold = True
- 列表：doc.add_paragraph("条目", style="List Bullet")（或 "List Number"）
- 表格填数：cell = table.rows[i].cells[j]; cell.text = "值"
- 读已有文档：doc = docx.Document("in.docx")，遍历 doc.paragraphs / doc.tables
- 中文默认字体：p.runs[0].font.name = "宋体"; 并设置 east asian：
  from docx.oxml.ns import qn; run._element.rPr.rFonts.set(qn("w:eastAsia"), "宋体")

## 习惯
1. 先把需求拆成小脚本，写入临时 .py 再执行（便于排错与重跑）。
2. 生成后打印 os.path.getsize 确认文件落盘。
3. 修改现有文档时先复制一份再改。