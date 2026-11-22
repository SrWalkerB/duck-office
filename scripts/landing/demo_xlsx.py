"""Planilha de demonstração (dados fictícios) para os prints da landing page.

Precisa do openpyxl (pip install openpyxl==3.1.5). Para refazer os prints:

    python3 scripts/landing/demo_xlsx.py target/demo.xlsx
    cargo run --release -p duck-sheets -- target/demo.xlsx --screenshot site/img/app.png
    cargo run --release -p duck-sheets -- target/demo.xlsx --zoom 150 --screenshot zoom.png
    cargo run --release -p duck-sheets -- target/demo.xlsx --zoom 150 --find sao --screenshot busca.png

zoom.png, condicional.png e busca.png são recortes dessas telas (veja site/img).
"""
import random
import sys

from openpyxl import Workbook
from openpyxl.formatting.rule import CellIsRule, ColorScaleRule, DataBarRule, IconSetRule
from openpyxl.styles import Alignment, Border, Font, PatternFill, Side

random.seed(7)
out = sys.argv[1]

AZUL = "1F3A68"
CINZA = "F1F3F6"
LINHA = "D9DDE3"
thin = Side(style="thin", color=LINHA)
medium = Side(style="medium", color="8A93A3")
double = Side(style="double", color="1D1D1F")

lojas = [
    ("Sudeste", "São Paulo · Paulista"), ("Sudeste", "São Paulo · Moema"), ("Sudeste", "Campinas"),
    ("Sudeste", "Rio de Janeiro · Centro"), ("Sudeste", "Belo Horizonte"),
    ("Sudeste", "São José dos Campos"), ("Sudeste", "Vitória"), ("Sul", "Curitiba"), ("Sul", "Porto Alegre"),
    ("Sul", "Florianópolis"), ("Sul", "Maringá"), ("Nordeste", "Salvador"),
    ("Nordeste", "Recife"), ("Nordeste", "Fortaleza"), ("Nordeste", "São Luís"), ("Nordeste", "Natal"),
    ("Centro-Oeste", "Brasília"), ("Centro-Oeste", "Goiânia"),
    ("Centro-Oeste", "Campo Grande"), ("Centro-Oeste", "Cuiabá"), ("Norte", "Manaus"), ("Norte", "Belém"),
    ("Norte", "Palmas"),
]

wb = Workbook()
ws = wb.active
ws.title = "Vendas 2026"
ws.sheet_properties.tabColor = "2F6BFF"
ws.sheet_view.showGridLines = True

cols = ["Região", "Loja", "Pedidos", "Faturamento", "Meta", "% da meta", "Ticket médio", "Variação", "Situação"]
widths = [17, 30, 13, 22, 19, 16, 17, 14, 20]
for i, w in enumerate(widths):
    ws.column_dimensions[chr(65 + i)].width = w

ws.merge_cells("A1:I1")
ws["A1"] = "Vendas 2026 · Rede Pato Feliz"
ws["A1"].font = Font(name="Calibri", size=18, bold=True, color="FFFFFF")
ws["A1"].fill = PatternFill("solid", fgColor=AZUL)
ws["A1"].alignment = Alignment(horizontal="left", vertical="center", indent=1)
ws.row_dimensions[1].height = 36
ws.merge_cells("A2:I2")
ws["A2"] = "Janeiro a outubro, por loja. Valores em reais. Dados fictícios."
ws["A2"].font = Font(name="Calibri", size=11, italic=True, color="5A6675")
ws["A2"].alignment = Alignment(horizontal="left", vertical="center", indent=1)
ws.row_dimensions[2].height = 22

for i, c in enumerate(cols):
    cell = ws.cell(row=3, column=i + 1, value=c)
    cell.font = Font(name="Calibri", size=11, bold=True, color="1D1D1F")
    cell.fill = PatternFill("solid", fgColor="E4E8EE")
    cell.alignment = Alignment(horizontal="center" if i >= 2 else "left", vertical="center", indent=0 if i >= 2 else 1)
    cell.border = Border(bottom=medium, top=thin, left=thin, right=thin)
ws.row_dimensions[3].height = 26

r0 = 4
tot = [0, 0.0, 0.0]
for k, (reg, loja) in enumerate(lojas):
    r = r0 + k
    pedidos = random.randint(1800, 14000)
    ticket = random.uniform(78, 162)
    fat = round(pedidos * ticket, 2)
    meta = round(fat / random.uniform(0.78, 1.22), -3)
    pct = fat / meta
    var = random.uniform(-0.14, 0.22)
    sit = "Acima da meta" if pct >= 1.03 else ("Na meta" if pct >= 0.95 else "Abaixo da meta")
    vals = [reg, loja, pedidos, fat, meta, pct, fat / pedidos, var, sit]
    fmts = [None, None, "#,##0", '"R$" #,##0.00', '"R$" #,##0', "0.0%", '"R$" #,##0.00', "+0.0%;-0.0%;0.0%", None]
    for i, v in enumerate(vals):
        c = ws.cell(row=r, column=i + 1, value=v)
        c.font = Font(name="Calibri", size=11, color="1D1D1F")
        if fmts[i]:
            c.number_format = fmts[i]
        c.border = Border(left=thin, right=thin, top=thin, bottom=thin)
        c.alignment = Alignment(vertical="center", indent=1 if i < 2 else 0, horizontal="center" if i == 8 else None)
        if k % 2 == 1:
            c.fill = PatternFill("solid", fgColor="F7F9FC")
    ws.row_dimensions[r].height = 21
    tot[0] += pedidos
    tot[1] += fat
    tot[2] += meta

rl = r0 + len(lojas) - 1
rt = rl + 1
ws.cell(row=rt, column=1, value="Total")
ws.cell(row=rt, column=3, value=tot[0]).number_format = "#,##0"
ws.cell(row=rt, column=4, value=round(tot[1], 2)).number_format = '"R$" #,##0.00'
ws.cell(row=rt, column=5, value=tot[2]).number_format = '"R$" #,##0'
ws.cell(row=rt, column=6, value=tot[1] / tot[2]).number_format = "0.0%"
ws.cell(row=rt, column=7, value=tot[1] / tot[0]).number_format = '"R$" #,##0.00'
for i in range(9):
    c = ws.cell(row=rt, column=i + 1)
    c.font = Font(name="Calibri", size=12, bold=True)
    c.border = Border(top=double, bottom=thin)
    c.alignment = Alignment(vertical="center", indent=1 if i < 2 else 0)
ws.row_dimensions[rt].height = 26

rng = lambda col: f"{col}{r0}:{col}{rl}"
ws.conditional_formatting.add(rng("D"), DataBarRule(start_type="min", end_type="max", color="7FA6F5", showValue=True))
ws.conditional_formatting.add(rng("F"), ColorScaleRule(start_type="num", start_value=0.8, start_color="F8696B",
                                                      mid_type="num", mid_value=1.0, mid_color="FFEB84",
                                                      end_type="num", end_value=1.2, end_color="63BE7B"))
ws.conditional_formatting.add(rng("H"), IconSetRule("3Arrows", "num", [-1, 0, 0.05], showValue=True))
ws.conditional_formatting.add(rng("H"), CellIsRule(operator="lessThan", formula=["0"], font=Font(color="C5221F")))
ws.conditional_formatting.add(rng("H"), CellIsRule(operator="greaterThanOrEqual", formula=["0"], font=Font(color="1E7B3A")))
ws.conditional_formatting.add(rng("I"), CellIsRule(operator="equal", formula=['"Acima da meta"'],
                                                  fill=PatternFill("solid", fgColor="DDF3E4", bgColor="DDF3E4"), font=Font(color="1E7B3A", bold=True)))
ws.conditional_formatting.add(rng("I"), CellIsRule(operator="equal", formula=['"Na meta"'],
                                                  fill=PatternFill("solid", fgColor="FFF4CC", bgColor="FFF4CC"), font=Font(color="8A6500", bold=True)))
ws.conditional_formatting.add(rng("I"), CellIsRule(operator="equal", formula=['"Abaixo da meta"'],
                                                  fill=PatternFill("solid", fgColor="FDE2E1", bgColor="FDE2E1"), font=Font(color="C5221F", bold=True)))


# Outras abas, para a barra de abas parecer de trabalho real
for name, color in [("Por região", "30A46C"), ("Produtos", "F2A900"), ("Metas 2027", "8F6BFF")]:
    s = wb.create_sheet(name)
    s.sheet_properties.tabColor = color
    s["A1"] = name
    s["A1"].font = Font(name="Calibri", size=16, bold=True)

wb.save(out)
print(out, len(lojas), "lojas, linhas", r0, "a", rt)
