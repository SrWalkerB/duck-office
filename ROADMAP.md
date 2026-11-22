# Roadmap do duck-office

Onde o projeto está, o que já foi feito e o que falta. Os detalhes de cada decisão e as medições completas ficam em [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

**Fase atual: fim da F3.5** (a casca do app): faixa de opções, menu Arquivo, clique direito, planilha nova, mesclar, ocultar, congelar painéis, "Abrir com", filtro automático, classificar e tabelas do Excel. Falta o teste manual de teclado e mouse. A próxima fase é o motor de cálculo (F4).

| Fase | Entrega | Status |
|---|---|---|
| **F0** | Camada nativa (`platform/`) e grade de 1 milhão de linhas | ✅ Concluída |
| **F1** | Leitura paralela de `.xlsx`/`.xlsm` e benchmark | ✅ Concluída (metas de 16+ núcleos em aberto) |
| **F2** | Visualizador `duck-sheets` | ✅ Concluída |
| **F3** | Edição e salvar com round-trip | ✅ Concluída (teste manual pendente) |
| **F3.5** | Casca do app: faixa de opções, menu Arquivo, clique direito, novo arquivo, "Abrir com"; filtro e classificação | ✅ Concluída (teste manual pendente) |
| **F4** | Motor de cálculo | ⏳ Próxima |
| **F5+** | Gráficos, pivôs, slicers, VBA, formato nativo, docs e slides | ⏳ Planejada |

---

## ✅ F0 · Camada nativa e grade de 1 milhão de linhas

- `duck-platform`: janela, teclado, mouse, DPI e clipboard (Windows, Linux e macOS).
- `duck-gfx`: renderer 2D por instâncias sobre wgpu, 1 draw call por frame, DX12 no Windows, Vulkan no Linux e Metal no macOS, renderização fora da tela para screenshots.
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

Resultado no arquivo de referência (185 MB, 1,9 GB de XML, 24 milhões de células, 10 milhões de fórmulas): **370–420 ms** para ler tudo com 16 threads (era 450–650 ms), pico de ~355 MB, **0 divergências** contra o calamine.

Em aberto: a meta de < 0,4 s está no limite, e o ganho ≥ 10× com 16+ núcleos não foi atingido. A leitura hoje está limitada pela CPU total; as alavancas que sobram são micro-otimizações do parse e do decodificador.

## ✅ F2 · Visualizador `duck-sheets`

Feito:

- **Abrir**: linha de comando, diálogo nativo (Ctrl+O), arrastar o arquivo para a janela; `.xlsx`, `.xlsm` e `.csv`.
- **Carga progressiva**: prévia da aba ativa → primeira tela → aba inteira → outras abas, cada uma quando fica pronta.
- **Aparência do Excel**:
  - formatos de número em pt-BR: moeda, contábil, %, datas e horas, científico, frações, cores e condições;
  - fontes (família, tamanho, negrito, itálico, sublinhado, cor), preenchimentos, cores do tema com tonalidade;
  - bordas (fina, média, grossa, dupla, pontilhada, tracejada), alinhamentos, recuo, texto quebrado;
  - larguras e alturas reais, linhas e colunas ocultas, texto que transborda para as vizinhas, `###`;
  - texto girado e empilhado, texto rico (várias fontes na célula), tachado, estilo de linha inteira;
  - formatação condicional: regras de valor, fórmula, texto e data, escalas de cor, barras de dados, ícones, N maiores, acima da média, repetidos.
- **Estrutura**: abas (ocultas ficam de fora, com a cor da guia), mesclagens, painéis congelados, barra de fórmulas, caixa de nome ("ir para").
- **Interação**: seleção com mouse, Shift e cabeçalhos; Ctrl+setas; copiar em TSV (Ctrl+C); soma, média e contagem da seleção; busca paralela sem acentos (Ctrl+F); zoom; redimensionar colunas e linhas arrastando o cabeçalho (duplo clique ajusta); hiperlinks com Ctrl+clique.
- **Plataforma**: crate novo `duck-ui` (Painter, campo de texto, abas), texto com estilos e quebra de linha, bordas tracejadas no shader.
- **Medição**: `--bench`, `--screenshot`, `duck-bench preview` e uma planilha de vitrine sem dados reais (`duck-bench gen-showcase`) no CI.

Resultado no Windows (AMD RX 9070 XT, DX12): **primeira tela com dados em 190–199 ms** (era 212–245), todas as 20 abas em 0,59–0,67 s, ~860–1.090 fps sem vsync (p99 do frame até 2,5 ms), **memória de 422–437 MB** com tudo carregado (era 657–663 MB) e **pico de 517–527 MB** durante a leitura (era 954–979 MB).

No Linux (WSLg, renderização por software): primeira tela em 125–182 ms, todas as abas em 0,75–0,95 s, frame montado em até 1,4 ms (p99), busca em 593 mil células em 33 ms, memória de 401–412 MB e pico de 467–489 MB (era 652–658 MB).

Para não atrasar a primeira tela, a GPU começa a abrir numa thread desde o início do processo (no DX12; a janela só cria a superfície), a leitura da planilha corre junto, e só 12 fontes essenciais são carregadas no início. As outras ~340 fontes do sistema são lidas numa thread depois da primeira tela; com o cache de disco frio, isso levava até 0,6 s.

Falta na F2:

- [x] Medir no Windows com a GPU (21/11/2026).
- [x] Memória abaixo de 600 MB: 423–437 MB no Windows, com os blocos de células compactados (dados: 396 → 193 MB).
- [x] Pico de memória durante a leitura: 517–527 MB no Windows e 475–489 MB no Linux (era ~960 e ~655 MB), com a leitura completa ~15% mais rápida.
- [x] Abrir a GPU antes da janela existir: primeira tela de 212–245 para 190–199 ms no Windows (no Linux a escolha do adaptador depende da janela e fica como estava).
- [ ] Teste manual de teclado e mouse na janela (redimensionar e Ctrl+clique em link ainda não foram usados à mão).
- [x] Texto girado (`textRotation`) e empilhado.
- [x] Formatação condicional (fora: regras `x14` do Excel 2010 e fórmulas com outras abas, intervalos ou funções fora do avaliador).
- [x] Estilo de linha inteira (`<row s>`).
- [x] Várias fontes na mesma célula (rich text) nas strings compartilhadas.
- [x] Redimensionar colunas e linhas com o mouse (duplo clique ajusta a coluna ao texto).
- [x] Hiperlinks (Ctrl+clique; externos só `http`, `https` e `mailto`).
- [x] Abrir `.csv` (separador detectado, vírgula decimal com `;`).

## ✅ F3 · Edição e salvar

- [x] **Salvar por remendo** (ADR-014): partes intactas copiadas com os bytes comprimidos originais; numa aba editada, só as linhas e células mudadas são reescritas, o resto é copiado byte a byte; compressão em paralelo.
- [x] **Conferência antes de substituir**: o arquivo é gravado num temporário, as abas regravadas são relidas e comparadas célula a célula com o modelo, e só então o original é trocado.
- [x] Camada de comandos com desfazer/refazer (`duck_core::Command`), inclusive passos que mexem em várias abas.
- [x] Editar células: na célula ou na barra de fórmulas; o que se digita é lido como no Excel em português (números, %, moeda, datas, horas, lógicos, fórmulas).
- [x] Apagar, copiar, recortar e colar (de outros programas também); colar dentro do app leva fórmulas (deslocadas) e estilos.
- [x] Formatação básica: negrito, itálico, sublinhado, tachado, cor do texto e do fundo, alinhamento, quebra de texto, formatos de número, bordas; larguras e alturas.
- [x] Inserir e apagar linhas e colunas, com as fórmulas de todas as abas, os nomes definidos, mesclagens, formatação condicional, validação e links ajustados.
- [x] `.csv` salvo com o mesmo separador e codificação.
- [x] `duck-bench roundtrip` (cópia intacta, reescrita total, edições, estrutura) no CI e no arquivo de referência: **0 divergências**.
- [ ] Teste manual de teclado e mouse na janela.

Resultado no arquivo de referência (Windows, 16 threads):

- salvar sem mudanças: **176–210 ms** (701 partes copiadas, idênticas byte a byte);
- 500 edições na aba de 1,6 GB de XML: gravação em **1,5 s** e conferência em 0,9 s; o arquivo cai de 185 para 129 MB (o `calcChain.xml` sai e o deflate é o nível 6);
- editar e salvar pelo app, abrindo o arquivo: **menos de 1 s** no total;
- inserir ou apagar linhas/colunas na aba de 593 mil linhas: **~1 s** cada; regravar a aba (1,6 GB de XML gerados do modelo em paralelo): 1,9 s + 0,9 s de conferência.

Limites desta versão:

- fórmulas novas só têm valor se o avaliador simples entender (sem intervalos como `SUM`); as que dependem da célula editada não mudam. O motor de cálculo vem na F4, e o arquivo salvo pede ao Excel para recalcular ao abrir;
- inserir/apagar linhas não move gráficos, imagens, tabelas e tabelas dinâmicas (o app avisa antes); na aba regravada, `cm`/`vm` das células e níveis de agrupamento das linhas não voltam;
- texto rico de células reescritas vira texto simples; quebra de linha dentro da célula (Alt+Enter) ainda não existe;
- os nomes das funções são os do arquivo (inglês, separador `,`).

## ⏳ F3.5 · Casca do app

Sem isso o editor só é usável por atalhos. O objetivo é que tudo o que a F3 já faz esteja à vista, como num editor de planilhas comum, e que o duck-sheets se integre ao sistema.

- [x] **Faixa de opções** com abas (Página Inicial, Inserir, Exibir) e grupos, no lugar da barra de ferramentas atual; atalhos rápidos (salvar, desfazer, refazer) no canto.
- [x] **Página Inicial**:
  - Área de transferência: colar, recortar, copiar;
  - Fonte: família e tamanho (listas), aumentar/diminuir, negrito, itálico, sublinhado, tachado, bordas, cor do fundo, cor do texto;
  - Alinhamento: cima/meio/baixo, esquerda/centro/direita, recuo, quebrar texto, mesclar e centralizar;
  - Número: lista de formatos (Geral, Número, Moeda, Contábil, Data, Hora, Porcentagem, Texto), moeda, %, milhar, mais/menos casas decimais;
  - Células: inserir e excluir (linhas, colunas), formatar (largura da coluna, altura da linha, ajustar, ocultar e reexibir);
  - Edição: limpar (tudo, formatos, conteúdo), localizar, ir para.
- [x] **Inserir**: linhas, colunas, mesclagem. **Exibir**: zoom, linhas de grade, cabeçalhos, congelar painéis (salvo no arquivo).
- [x] **Menu Arquivo**: novo, abrir, arquivos recentes, salvar, salvar como, "Abrir com", sair.
- [x] **Novo arquivo**: planilha em branco que pede onde salvar na primeira vez.
- [x] **Menu de clique direito** na grade e nos cabeçalhos (recortar, copiar, colar, inserir, excluir, limpar, mesclar, largura/altura, ocultar).
- [x] **Gravador**: mesclagens, linhas/colunas ocultas, formatos de número novos e painéis congelados também saem no arquivo salvo.
- [x] **Integração com o sistema**: registrar o duck-sheets no "Abrir com" de `.xlsx`/`.xlsm`/`.csv` (Windows e Linux), feito só quando o usuário pedir pelo menu.
- [ ] Teste manual de teclado e mouse na janela.

Segunda parte (aba **Dados**):

- [x] **Filtro automático**: as setas nos cabeçalhos (`autoFilter` da aba e das tabelas do Excel), lista de valores com busca e marcar/desmarcar (calculada em paralelo, só com as linhas que passam nos filtros das outras colunas); o resultado esconde as linhas como no Excel e é salvo no arquivo. Critérios que o Excel grava e o duck-sheets ainda não edita (personalizado, 10 maiores, por cor, por data) ficam guardados como estão.
- [x] **Classificar** pela coluna da célula ativa (crescente/decrescente), dentro do filtro, da tabela ou da região de dados, em paralelo e com desfazer. Classificar por várias colunas ao mesmo tempo fica para depois.
- [x] Desenho das **tabelas do Excel** (cabeçalho, faixas alternadas) a partir do estilo da tabela e das cores do tema; editar o cabeçalho atualiza o nome da coluna, e inserir/apagar linhas e colunas ajusta a tabela (o Excel exige os dois em dia).

Critério de pronto: dá para abrir, editar, formatar, filtrar e salvar uma planilha só com o mouse.

## ⏳ F4 · Motor de cálculo

- [ ] Parser de fórmulas e AST.
- [ ] Grafo de dependências por intervalos.
- [ ] Recálculo incremental, em níveis paralelos; fórmulas compartilhadas como kernels SIMD.
- [ ] Funções mais usadas (matemáticas, texto, datas, busca, lógicas), com compatibilidade numérica com o Excel.

Critério de pronto: recalcular o arquivo de referência sem travar a janela.

## ⏳ F5+ · O resto da suíte

- [ ] Gráficos, imagens e formas.
- [ ] Tabelas dinâmicas interativas, segmentação de dados (slicers) e linhas do tempo (timelines).
- [ ] VBA (hoje só preservado).
- [ ] Formato de arquivo nativo (ADR-013, em discussão).
- [x] macOS: compila, roda e passa no CI (Metal); o pacote `.app` assinado ainda não existe.
- [ ] `duck-docs` e `duck-slides` sobre a mesma camada `platform/`.

## Questões em aberto

- Licença do projeto.
- Nome próprio para a biblioteca de `platform/`.
- Corpus de testes com planilhas públicas grandes e variadas.
