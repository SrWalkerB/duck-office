"""Planilha de demonstração (dados fictícios) para os prints da landing page.

Precisa do openpyxl (pip install openpyxl==3.1.5). Para refazer os prints:

    python3 scripts/landing/demo_xlsx.py target/demo.xlsx
    cargo run --release -p duck-sheets -- target/demo.xlsx --screenshot site/img/app.png
    cargo run --release -p duck-sheets -- target/demo.xlsx --sheet 6 --screenshot site/img/painel.png
    cargo run --release -p duck-sheets -- target/demo.xlsx --zoom 150 --screenshot zoom.png
    cargo run --release -p duck-sheets -- target/demo.xlsx --zoom 150 --find sao --screenshot busca.png

zoom.png, condicional.png e busca.png são recortes dessas telas (veja site/img).
"""
import datetime
import random
import re
import shutil
import sys
import zipfile

from openpyxl import Workbook
from openpyxl.formatting.rule import CellIsRule, ColorScaleRule, DataBarRule, IconSetRule
from openpyxl.styles import Alignment, Border, Font, PatternFill, Side
from openpyxl.worksheet.filters import AutoFilter
from openpyxl.worksheet.table import Table, TableStyleInfo

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

# Aba cheia, para a busca: um pedido por linha.
q = wb.create_sheet("Pedidos")
q.sheet_properties.tabColor = "0E9AA7"
q.freeze_panes = "A2"
CABECALHO = ["Pedido", "Data", "Região", "Loja", "Vendedor", "Produto", "Categoria", "Canal", "Qtde", "Preço", "Desconto", "Total", "Custo", "Margem", "Entrega"]
NOMES = ["Ana", "Bruno", "Carla", "Davi", "Elisa", "Fábio", "Gil", "Helena", "Igor", "Joana", "Kauê", "Lia", "Márcio", "Nina", "Otávio"]
PRODUTOS = [("Notebook 14", "Informática", 3890), ("Monitor 27", "Informática", 1490), ("Teclado", "Acessórios", 289), ("Cadeira", "Móveis", 1190),
            ("Mesa ajustável", "Móveis", 2350), ("Fone sem fio", "Áudio", 459), ("Caixa de som", "Áudio", 690), ("Webcam", "Acessórios", 349)]
for i, (h, w) in enumerate(zip(CABECALHO, [11, 12, 14, 25, 12, 17, 14, 12, 8, 13, 11, 14, 13, 10, 12])):
    c = q.cell(row=1, column=i + 1, value=h)
    c.font = Font(name="Calibri", size=11, bold=True, color="FFFFFF")
    c.fill = PatternFill("solid", fgColor=AZUL)
    q.column_dimensions[chr(65 + i)].width = w
DIA0 = datetime.date(2026, 1, 1)
FMT = {2: "dd/mm/yyyy", 10: '"R$" #,##0.00', 11: "0%", 12: '"R$" #,##0.00', 13: '"R$" #,##0.00', 14: "0.0%", 15: "dd/mm/yyyy"}
for k in range(60_000):
    reg, loja = random.choice(lojas)
    prod, cat, preco = random.choice(PRODUTOS)
    qt, desc = random.randint(1, 9), random.choice([0, 0, 0, 0.05, 0.1, 0.15])
    dia = DIA0 + datetime.timedelta(days=k * 300 // 60_000)
    tot_l, custo = round(qt * preco * (1 - desc), 2), round(qt * preco * random.uniform(0.5, 0.72), 2)
    q.append([100001 + k, dia, reg, loja, random.choice(NOMES), prod, cat, random.choice(["Loja", "Site", "Aplicativo"]), qt, preco, desc,
              tot_l, custo, (tot_l - custo) / tot_l, dia + datetime.timedelta(days=random.randint(1, 9))])
    for c, f in FMT.items():
        q.cell(row=k + 2, column=c).number_format = f

# ---------------------------------------------------------------- Painel

MESES = ["jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out"]
CANAIS = ["Loja", "Site", "Aplicativo"]
ESCOLHIDAS = ["Sudeste", "Sul"]  # o que está marcado na segmentação "Região"

p = wb.create_sheet("Painel")
p.sheet_properties.tabColor = "E5484D"
p.sheet_view.showGridLines = False
for col, w in zip("ABCDEFGHIJKLMNOPQRS", [2, 8, 14, 25, 12, 10, 17] + [11] * 12):
    p.column_dimensions[col].width = w
for c in range(1, 20):
    p.cell(row=1, column=c).fill = PatternFill("solid", fgColor=AZUL)
p["B1"] = "Painel de vendas · Rede Pato Feliz"
p["B1"].font = Font(name="Calibri", size=18, bold=True, color="FFFFFF")
p["B1"].alignment = Alignment(vertical="center")
p.row_dimensions[1].height = 36
p.row_dimensions[2].height = 12

regs = []
for mes in MESES:
    for reg, loja in lojas:
        for canal in CANAIS:
            pedidos = random.randint(60, 2400)
            regs.append((mes, reg, loja, canal, pedidos, round(pedidos * random.uniform(78, 162), 2)))
visiveis = [r for r in regs if r[1] in ESCOLHIDAS]
total = round(sum(r[5] for r in visiveis), 2)

# Indicadores: fórmulas que acompanham o filtro da tabela.
FORMULAS = [
    (3, "Faturamento", "SUBTOTAL(109,Vendas[Faturamento])", total),
    (4, "Pedidos", "SUBTOTAL(109,Vendas[Pedidos])", sum(r[4] for r in visiveis)),
]
for r, rotulo, formula, valor in FORMULAS:
    a = p.cell(row=r, column=2, value=rotulo)
    a.font = Font(name="Calibri", size=11, color="5A6675")
    a.alignment = Alignment(vertical="center")
    b = p.cell(row=r, column=7, value="=" + formula)
    b.number_format = '"R$" #,##0' if r == 3 else "#,##0"
    b.font = Font(name="Calibri", size=14 if r == 3 else 12, bold=True, color=AZUL)
    b.alignment = Alignment(vertical="center", horizontal="right")
    p.row_dimensions[r].height = 24

t0 = 6
p.row_dimensions[5].height = 12
for i, h in enumerate(["Mês", "Região", "Loja", "Canal", "Pedidos", "Faturamento"]):
    p.cell(row=t0, column=2 + i, value=h)
p.row_dimensions[t0].height = 24
for k, rec in enumerate(regs):
    r = t0 + 1 + k
    for i, v in enumerate(rec):
        c = p.cell(row=r, column=2 + i, value=v)
        c.font = Font(name="Calibri", size=11, color="1D1D1F")
        c.alignment = Alignment(vertical="center")
        if i >= 4:
            c.number_format = '"R$" #,##0.00' if i == 5 else "#,##0"
    p.row_dimensions[r].height = 20
    p.row_dimensions[r].hidden = rec[1] not in ESCOLHIDAS
fim = t0 + len(regs)
tab = Table(displayName="Vendas", ref=f"B{t0}:G{fim}")
tab.autoFilter = AutoFilter(ref=f"B{t0}:G{fim}")
tab.autoFilter.add_filter_column(1, ESCOLHIDAS)
tab.tableStyleInfo = TableStyleInfo(name="TableStyleMedium2", showRowStripes=True)
p.add_table(tab)
p.sheet_view.selection[0].activeCell = p.sheet_view.selection[0].sqref = "G3"

# Dados dos gráficos, numa aba à parte (as linhas do Painel somem com o filtro).
d = wb.create_sheet("Dados do painel")
d.append(["Mês", "Faturamento", "Meta"])
for m in MESES:
    f = round(sum(r[5] for r in visiveis if r[0] == m), 2)
    d.append([m, f, round(total / 10 * random.uniform(0.92, 1.06), -3)])
d["E1"], d["F1"] = "Canal", "Faturamento"
for k, canal in enumerate(CANAIS):
    d.cell(row=2 + k, column=5, value=canal)
    d.cell(row=2 + k, column=6, value=round(sum(r[5] for r in visiveis if r[3] == canal), 2))
por_loja = {}
for r in visiveis:
    por_loja[r[2]] = por_loja.get(r[2], 0) + r[5]
d["H1"], d["I1"] = "Loja", "Faturamento"
for k, (loja, v) in enumerate(sorted(por_loja.items(), key=lambda x: x[1])[-8:]):
    d.cell(row=2 + k, column=8, value=loja)
    d.cell(row=2 + k, column=9, value=round(v, 2))

wb.save(out)
coluna = {c.name: c.id for c in tab.tableColumns}  # as segmentações apontam pelo id da coluna

# ---------------------------------------------------------------- objetos do Painel
# O openpyxl não grava segmentações; os objetos entram direto no zip.

MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
DML = "http://schemas.openxmlformats.org/drawingml/2006"
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
PKG = "http://schemas.openxmlformats.org/package/2006/relationships"
MS07 = "http://schemas.microsoft.com/office/2007/relationships"
SLE = "http://schemas.microsoft.com/office/drawing"
X14 = "http://schemas.microsoft.com/office/spreadsheetml/2009/9/main"
X15 = "http://schemas.microsoft.com/office/spreadsheetml/2010/11/main"
XML = '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
ABA = "'Dados do painel'!"


def cor(hexa):
    return f'<a:solidFill><a:srgbClr val="{hexa}"/></a:solidFill>'


def serie(i, nome, cat, val, sppr, extra=""):
    """`cat` e `val` são intervalos absolutos da aba de dados."""
    return (f'<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:strRef><c:f>{ABA}{nome}</c:f></c:strRef></c:tx>'
            f'<c:spPr>{sppr}</c:spPr>{extra}<c:cat><c:strRef><c:f>{ABA}{cat}</c:f></c:strRef></c:cat>'
            f'<c:val><c:numRef><c:f>{ABA}{val}</c:f></c:numRef></c:val></c:ser>')


def eixos(a, b, maior, horizontal=False):
    pos = ("l", "b") if horizontal else ("b", "l")
    return (f'<c:catAx><c:axId val="{a}"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/>'
            f'<c:axPos val="{pos[0]}"/><c:crossAx val="{b}"/></c:catAx>'
            f'<c:valAx><c:axId val="{b}"/><c:scaling><c:orientation val="minMax"/><c:max val="{maior * 1.12:.0f}"/><c:min val="0"/></c:scaling><c:delete val="0"/>'
            f'<c:axPos val="{pos[1]}"/><c:majorGridlines><c:spPr><a:ln w="9525">{cor("E4E8EE")}</a:ln></c:spPr></c:majorGridlines>'
            f'<c:numFmt formatCode="#,##0.0,,&quot; mi&quot;" sourceLinked="0"/><c:crossAx val="{a}"/></c:valAx>')


def grafico(titulo, plot, legenda):
    leg = f'<c:legend><c:legendPos val="{legenda}"/><c:overlay val="0"/></c:legend>' if legenda else ""
    return (f'{XML}<c:chartSpace xmlns:c="{DML}/chart" xmlns:a="{DML}/main" xmlns:r="{REL}"><c:chart>'
            f'<c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr sz="1300" b="1">{cor("1D1D1F")}</a:rPr><a:t>{titulo}</a:t></a:r></a:p>'
            f'</c:rich></c:tx><c:overlay val="0"/></c:title><c:autoTitleDeleted val="0"/>'
            f'<c:plotArea><c:layout/>{plot}</c:plotArea>{leg}<c:plotVisOnly val="1"/></c:chart>'
            f'<c:spPr>{cor("FFFFFF")}<a:ln w="9525">{cor("D9DDE3")}</a:ln></c:spPr></c:chartSpace>')


pontos = "".join(f'<c:dPt><c:idx val="{i}"/><c:bubble3D val="0"/><c:spPr>{cor(c)}</c:spPr></c:dPt>'
                 for i, c in enumerate(["2F6BFF", "8F6BFF", "FFB01F"]))
graficos = [
    grafico("Faturamento e meta por mês",
            '<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
            + serie(0, "$B$1", "$A$2:$A$11", "$B$2:$B$11", cor("2F6BFF"))
            + '<c:gapWidth val="60"/><c:axId val="1"/><c:axId val="2"/></c:barChart>'
            + '<c:lineChart><c:grouping val="standard"/><c:varyColors val="0"/>'
            + serie(1, "$C$1", "$A$2:$A$11", "$C$2:$C$11", f'<a:ln w="28575">{cor("FFB01F")}</a:ln>',
                    '<c:marker><c:symbol val="circle"/></c:marker>')
            + '<c:marker val="1"/><c:axId val="1"/><c:axId val="2"/></c:lineChart>' + eixos(1, 2, total / 10 * 1.12), "b"),
    grafico("Por canal",
            '<c:doughnutChart><c:varyColors val="1"/>'
            + serie(0, "$F$1", "$E$2:$E$4", "$F$2:$F$4", "", pontos)
            + '<c:firstSliceAng val="0"/><c:holeSize val="58"/></c:doughnutChart>', "r"),
    grafico("As 8 maiores lojas",
            '<c:barChart><c:barDir val="bar"/><c:grouping val="clustered"/><c:varyColors val="0"/>'
            + serie(0, "$I$1", "$H$2:$H$9", "$I$2:$I$9", cor("30A46C"))
            + '<c:gapWidth val="45"/><c:axId val="5"/><c:axId val="6"/></c:barChart>' + eixos(5, 6, max(por_loja.values()), True), ""),
]


def ancora(x, y, w, h, corpo):
    """Posição absoluta em pixels: não anda quando o filtro esconde linhas."""
    e = 9525
    return (f'<xdr:absoluteAnchor><xdr:pos x="{x * e}" y="{y * e}"/><xdr:ext cx="{w * e}" cy="{h * e}"/>'
            f'{corpo}<xdr:clientData/></xdr:absoluteAnchor>')


def quadro(i, nome, uri, dado):
    return (f'<xdr:graphicFrame macro=""><xdr:nvGraphicFramePr><xdr:cNvPr id="{i}" name="{nome}"/><xdr:cNvGraphicFramePr/>'
            f'</xdr:nvGraphicFramePr><xdr:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/></xdr:xfrm>'
            f'<a:graphic><a:graphicData uri="{uri}">{dado}</a:graphicData></a:graphic></xdr:graphicFrame>')


def q_grafico(i):
    return quadro(i + 2, f"Gráfico {i + 1}", f"{DML}/chart", f'<c:chart xmlns:c="{DML}/chart" r:id="rId{i + 1}"/>')


def q_segmentacao(i, nome):
    q = quadro(i, nome, f"{SLE}/2010/slicer", f'<sle:slicer name="{nome}"/>')
    return f'<mc:AlternateContent><mc:Choice Requires="sle15">{q}</mc:Choice><mc:Fallback/></mc:AlternateContent>'


X, L = 690, 680
desenho = (f'{XML}<xdr:wsDr xmlns:xdr="{DML}/spreadsheetDrawing" xmlns:a="{DML}/main" xmlns:r="{REL}"'
           f' xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:sle="{SLE}/2010/slicer" xmlns:sle15="{SLE}/2012/slicer">'
           + ancora(X, 62, 420, 74, q_segmentacao(20, "Região")) + ancora(X + 432, 62, L - 432, 74, q_segmentacao(21, "Canal"))
           + ancora(X, 148, L, 236, q_grafico(0))
           + ancora(X, 396, 270, 224, q_grafico(1)) + ancora(X + 282, 396, L - 282, 224, q_grafico(2)) + "</xdr:wsDr>")


def rels(lista):
    return XML + f'<Relationships xmlns="{PKG}">' + "".join(
        f'<Relationship Id="{i}" Type="{t}" Target="{alvo}"/>' for i, t, alvo in lista) + "</Relationships>"


def cache(nome, coluna):
    return (f'{XML}<slicerCacheDefinition xmlns="{X14}" xmlns:x="{MAIN}" name="SegmentaçãodeDados_{nome}" sourceName="{nome}"><extLst>'
            f'<x:ext uri="{{2F2917AC-EB37-4324-AD4E-5DD8C200BD13}}" xmlns:x15="{X15}">'
            f'<x15:tableSlicerCache tableId="1" column="{coluna}"/></x:ext></extLst></slicerCacheDefinition>')


def segmentacao(nome, colunas, estilo):
    return (f'<slicer name="{nome}" cache="SegmentaçãodeDados_{nome}" caption="{nome}" columnCount="{colunas}"'
            f' style="{estilo}" rowHeight="241300"/>')


novos = {
    "xl/drawings/drawing1.xml": desenho,
    "xl/drawings/_rels/drawing1.xml.rels": rels(
        [(f"rId{i + 1}", f"{REL}/chart", f"/xl/charts/chart{i + 1}.xml") for i in range(len(graficos))]),
    "xl/slicers/slicer1.xml": f'{XML}<slicers xmlns="{X14}">' + segmentacao("Região", 5, "SlicerStyleDark1")
                              + segmentacao("Canal", 3, "SlicerStyleDark4") + "</slicers>",
    "xl/slicerCaches/slicerCache1.xml": cache("Região", coluna["Região"]),
    "xl/slicerCaches/slicerCache2.xml": cache("Canal", coluna["Canal"]),
}
novos.update({f"xl/charts/chart{i + 1}.xml": g for i, g in enumerate(graficos)})
TIPOS = {"drawings": "application/vnd.openxmlformats-officedocument.drawing+xml",
         "charts": "application/vnd.openxmlformats-officedocument.drawingml.chart+xml",
         "slicers": "application/vnd.ms-excel.slicer+xml", "slicerCaches": "application/vnd.ms-excel.slicerCache+xml"}


def remendo(nome, b):
    if nome == "xl/worksheets/sheet6.xml":
        for _, _, f, v in FORMULAS:
            b, n = re.subn(re.escape(f"<f>{f}</f>") + r"\s*(<v\s*/>|<v></v>)", f"<f>{f}</f><v>{v!r}</v>", b)
            assert n == 1, f
        ext = (f'<extLst><ext uri="{{3A4CF648-6AED-40f4-86FF-DC5316D8AED3}}" xmlns:x14="{X14}">'
               f'<x14:slicerList><x14:slicer xmlns:r="{REL}" r:id="rIdSeg"/></x14:slicerList></ext></extLst>')
        i = b.index("<tableParts")
        b = b[:i] + f'<drawing xmlns:r="{REL}" r:id="rIdDes"/>' + b[i:].replace("</worksheet>", ext + "</worksheet>")
    elif nome == "xl/worksheets/_rels/sheet6.xml.rels":
        b = b.replace("</Relationships>",
                      f'<Relationship Id="rIdDes" Type="{REL}/drawing" Target="/xl/drawings/drawing1.xml"/>'
                      f'<Relationship Id="rIdSeg" Type="{MS07}/slicer" Target="/xl/slicers/slicer1.xml"/></Relationships>')
    elif nome == "xl/_rels/workbook.xml.rels":
        b = b.replace("</Relationships>", "".join(
            f'<Relationship Id="rIdC{i}" Type="{MS07}/slicerCache" Target="/xl/slicerCaches/slicerCache{i}.xml"/>' for i in (1, 2))
            + "</Relationships>")
    elif nome == "xl/workbook.xml":
        nomes = "".join(f'<definedName name="SegmentaçãodeDados_{x}">#N/A</definedName>' for x in ("Canal", "Região"))
        b = re.sub(r"<definedNames\s*/>", "", b).replace("<calcPr", f"<definedNames>{nomes}</definedNames><calcPr")
        b = b.replace("</workbook>",
                      f'<extLst><ext uri="{{46BE6895-7355-4a93-B00E-2C351335B9C9}}" xmlns:x15="{X15}"><x15:slicerCaches xmlns:x14="{X14}">'
                      f'<x14:slicerCache xmlns:r="{REL}" r:id="rIdC1"/><x14:slicerCache xmlns:r="{REL}" r:id="rIdC2"/>'
                      f'</x15:slicerCaches></ext></extLst></workbook>')
    elif nome == "[Content_Types].xml":
        b = b.replace("</Types>", "".join(f'<Override PartName="/{a}" ContentType="{TIPOS[a.split("/")[1]]}"/>'
                                          for a in novos if "_rels" not in a) + "</Types>")
    return b


tmp = out + ".tmp"
with zipfile.ZipFile(out) as zi, zipfile.ZipFile(tmp, "w", zipfile.ZIP_DEFLATED) as zo:
    for n in zi.namelist():
        zo.writestr(n, remendo(n, zi.read(n).decode("utf8")))
    for n, b in novos.items():
        zo.writestr(n, b)
shutil.move(tmp, out)
print(out, len(lojas), "lojas, linhas", r0, "a", rt)
