---
name: office-sheets
description: 用捆绑的 Python（openpyxl）创建和编辑 Excel 表格
triggers: excel, xlsx, 表格, 电子表, 报表, 统计
---
# Excel 表格（openpyxl）

> Python 调用约定：先 ``python -c "import docx"`` 试探；报错说明系统 python 缺文档库，改用 Wish 自带的 ``D:\Wish\python\python.exe``（文档库只装在自带环境）。
Wish 自带 Python（PATH 上直接 python；不在则 D:\Wish\python\python.exe）。

## 快速上手
```python
import openpyxl
from openpyxl.styles import Font, PatternFill
wb = openpyxl.Workbook()
ws = wb.active
ws.title = "数据"
ws["A1"] = "名称"; ws["B1"] = "数量"
ws.append(["条目一", 10])
ws["B1"].font = Font(bold=True, color="FFFFFF")
ws["B1"].fill = PatternFill("solid", fgColor="A64A33")
ws.column_dimensions["A"].width = 22
ws.freeze_panes = "A2"
wb.save("out.xlsx")
```

## 常用手法
- 公式：ws["C2"] = "=SUM(B2:B100)"
- 批量写：for i, row in enumerate(data, start=2): ws.append(row)
- 读：wb = openpyxl.load_workbook("in.xlsx", data_only=True)（data_only 取公式计算值）
- 多表单：wb.create_sheet("汇总")
- 条件格式/图表建议直接生成后用说明文档描述；复杂图表可换 XlsxWriter（已捆绑）。

## 习惯
1. 大数据先打印 len(data) 核对行数。
2. 写完用 openpyxl 重新 load 一次抽验关键单元格。