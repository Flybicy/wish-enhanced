---
name: office-slides
description: 用捆绑的 Python（python-pptx）生成 PowerPoint 幻灯片
triggers: ppt, pptx, powerpoint, 幻灯片, 演示, 汇报, slides
---
# PowerPoint（python-pptx）

> Python 调用约定：先 ``python -c "import docx"`` 试探；报错说明系统 python 缺文档库，改用 Wish 自带的 ``D:\Wish\python\python.exe``（文档库只装在自带环境）。
Wish 自带 Python（PATH 上直接 python；不在则 D:\Wish\python\python.exe）。

## 快速上手
```python
from pptx import Presentation
from pptx.util import Inches, Pt
prs = Presentation()
slide = prs.slides.add_slide(prs.slide_layouts[0])  # 0=标题页 1=标题+内容 5=仅标题 6=空白
slide.shapes.title.text = "汇报标题"
slide.placeholders[1].text = "副标题"
s2 = prs.slides.add_slide(prs.slide_layouts[1])
s2.shapes.title.text = "要点"
body = s2.placeholders[1].text_frame
body.text = "第一点"
p = body.add_paragraph(); p.text = "第二点"; p.level = 1
box = s2.shapes.add_textbox(Inches(1), Inches(4), Inches(8), Inches(1))
box.text_frame.text = "附注文字"
prs.save("out.pptx")
```

## 常用手法
- 配色：shape.fill.fore_color.rgb = RGBColor(0xA6, 0x4A, 0x33)；文字色 run.font.color.rgb
- 字号：run.font.size = Pt(28)；加粗 run.font.bold = True
- 表格：s.shapes.add_table(rows, cols, Inches(x), Inches(y), Inches(w), Inches(h))
- 图片：s.shapes.add_picture("img.png", Inches(1), Inches(1))

## 习惯
1. 页数多时用 for 循环 + 内容清单生成，别逐页复制代码。
2. 生成后打印 len(prs.slides.__iter__.__self__._sldIdLst) 之类不必——直接重新打开数页数验证。