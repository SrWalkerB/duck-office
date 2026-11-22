# Roadmap do duck-office

Onde o projeto está, o que já foi feito e o que falta. Os detalhes de cada decisão e as medições completas ficam em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

**Fase atual: fim da F2.** O visualizador está pronto e medido no Windows e no Linux. A próxima fase é a **F3** (edição e salvar).

| Fase | Entrega | Status |
|---|---|---|
| **F0** | Camada nativa (`platform/`) e grade de 1 milhão de linhas | ✅ Concluída |
| **F1** | Leitura paralela de `.xlsx`/`.xlsm` e benchmark | ✅ Concluída (metas de 16+ núcleos em aberto) |
| **F2** | Visualizador `duck-sheets` | ✅ Concluída |
| **F3** | Edição e salvar com round-trip | ⏳ Próxima |
| **F4** | Motor de cálculo | ⏳ Planejada |
| **F5+** | Gráficos, pivôs, slicers, VBA, formato nativo, docs e slides | ⏳ Planejada |

---

## ✅ F0 · Camada nativa e grade de 1 milhão de linhas

- `duck-platform`: janela, teclado, mouse, DPI e clipboard (Windows e Linux).
- `duck-gfx`: renderer 2D por instâncias sobre wgpu, 1 draw call por frame, DX12 no Windows e Vulkan no Linux, renderização fora da tela para screenshots.
- `duck-text`: shaping, atlas de glifos, cache de texto e caminho rápido para números.
- `million_rows`: grade de 1 a 10 milhões de linhas.
- Regra no CI: `platform/` não depende de nada da suíte.

Resultado: ~0,8 ms de CPU por frame (1.150+ fps sem vsync) no Windows, e o mesmo custo com 10 milhões de linhas.

## ✅ F1 · Leitura paralela de planilhas

- `duck-core`: modelo colunar em blocos (11 bytes por célula na montagem, compactados no fim), strings contíguas, estilos.
- `duck-par`: pool de threads, limite de memória em trânsito, cortes por SIMD.
- `duck-deflate`: descompactação deflate **paralela e especulativa** (decodificador próprio).
- `duck-xlsx`: todas as abas ao mesmo tempo, abas grandes em pedaços paralelos, fórmulas (compartilhadas e deduplicadas), larguras, alturas, mesclagens e painéis.
- `duck-calc`: texto das fórmulas, modelos relativos.
- `duck-bench`: `open`, `sweep`, `verify` (contra o calamine), `gen`, `inflate`, `parse`.

Resultado no arquivo de referência (185 MB, 1,9 GB de XML, 24 milhões de células, 10 milhões de fórmulas): **450–650 ms** para ler tudo, ~460 MB de memória, **0 divergências** contra o calamine.

Em aberto: as metas de < 0,4 s e ganho ≥ 10× com 16+ núcleos. A leitura hoje está limitada pela CPU total; as alavancas que sobram são micro-otimizações do parse e do decodificador.

## ✅ F2 · Visualizador `duck-sheets`

Feito:

- **Abrir**: linha de comando, diálogo nativo (Ctrl+O), arrastar o arquivo para a janela.
- **Carga progressiva**: prévia da aba ativa → primeira tela → aba inteira → outras abas, cada uma quando fica pronta.
- **Aparência do Excel**:
  - formatos de número em pt-BR: moeda, contábil, %, datas e horas, científico, frações, cores e condições;
  - fontes (família, tamanho, negrito, itálico, sublinhado, cor), preenchimentos, cores do tema com tonalidade;
  - bordas (fina, média, grossa, dupla, pontilhada, tracejada), alinhamentos, recuo, texto quebrado;
  - larguras e alturas reais, linhas e colunas ocultas, texto que transborda para as vizinhas, `###`.
- **Estrutura**: abas (ocultas ficam de fora, com a cor da guia), mesclagens, painéis congelados, barra de fórmulas, caixa de nome ("ir para").
- **Interação**: seleção com mouse, Shift e cabeçalhos; Ctrl+setas; copiar em TSV (Ctrl+C); soma, média e contagem da seleção; busca paralela sem acentos (Ctrl+F); zoom.
- **Plataforma**: crate novo `duck-ui` (Painter, campo de texto, abas), texto com estilos e quebra de linha, bordas tracejadas no shader.
- **Medição**: `--bench`, `--screenshot`, `duck-bench preview` e uma planilha de vitrine sem dados reais (`duck-bench gen-showcase`) no CI.

Resultado no Windows (AMD RX 9070 XT, DX12): **primeira tela com dados em 212–245 ms**, todas as 20 abas em 0,61–0,65 s, ~980–1.160 fps sem vsync (p99 do frame até 2,3 ms), **memória de 423–437 MB** com tudo carregado (era 657–663 MB) e **pico de 517–527 MB** durante a leitura (era 954–979 MB).

No Linux (WSLg, renderização por software): primeira tela em 125–182 ms, todas as abas em 0,75–0,95 s, frame montado em até 1,2 ms (p99), busca em 593 mil células em 33 ms, memória de 401–412 MB e pico de 475–489 MB (era 652–658 MB).

Para não atrasar a primeira tela, a leitura da planilha começa junto com a abertura da GPU, e só 12 fontes essenciais são carregadas no início. As outras ~340 fontes do sistema são lidas numa thread depois da primeira tela; com o cache de disco frio, isso levava até 0,6 s.

Falta na F2:

- [x] Medir no Windows com a GPU (21/11/2026).
- [x] Memória abaixo de 600 MB: 423–437 MB no Windows, com os blocos de células compactados (dados: 396 → 193 MB).
- [x] Pico de memória durante a leitura: 517–527 MB no Windows e 475–489 MB no Linux (era ~960 e ~655 MB), com a leitura completa ~15% mais rápida.
- [ ] Abrir a GPU antes da janela existir (a GPU é o que resta no caminho da primeira tela: ~200 ms).
- [ ] Teste manual de teclado e mouse na janela.
- [ ] Texto girado (`textRotation`), comum em painéis.
- [ ] Formatação condicional.
- [ ] Estilo de linha inteira (`<row s>`).
- [ ] Várias fontes na mesma célula (rich text).
- [ ] Redimensionar colunas e linhas com o mouse.
- [ ] Hiperlinks.
- [ ] Abrir `.csv` (opcional).

## ⏳ F3 · Edição e salvar

- [ ] Camada de comandos (base para desfazer/refazer, edição e, no futuro, VBA).
- [ ] Editar células: valores, texto, fórmulas (texto apenas; o cálculo vem na F4).
- [ ] Inserir e apagar linhas e colunas; colar.
- [ ] Mudar formatação básica (fonte, cor, preenchimento, formato de número, bordas, larguras).
- [ ] **Escrita** de `.xlsx`/`.xlsm` em paralelo, preservando intactas as partes que não entendemos (VBA, pivôs, gráficos, slicers).
- [ ] Teste de round-trip: abrir e salvar sem mudar nada deve dar um arquivo equivalente (conferido célula a célula).

Critério de pronto: round-trip verde num corpus de planilhas.

## ⏳ F4 · Motor de cálculo

- [ ] Parser de fórmulas e AST.
- [ ] Grafo de dependências por intervalos.
- [ ] Recálculo incremental, em níveis paralelos; fórmulas compartilhadas como kernels SIMD.
- [ ] Funções mais usadas (matemáticas, texto, datas, busca, lógicas), com compatibilidade numérica com o Excel.

Critério de pronto: recalcular o arquivo de referência sem travar a janela.

## ⏳ F5+ · O resto da suíte

- [ ] Gráficos, imagens e formas.
- [ ] Tabelas dinâmicas interativas, slicers e timelines.
- [ ] VBA (hoje só preservado).
- [ ] Formato de arquivo nativo (ADR-013, em discussão).
- [ ] macOS.
- [ ] `duck-docs` e `duck-slides` sobre a mesma camada `platform/`.

## Questões em aberto

- Licença do projeto.
- Nome próprio para a biblioteca de `platform/`.
- Corpus de testes com planilhas públicas grandes e variadas.
