# ROADMAP — Encurtador de URL em Rust

Regras de progressão: uma etapa por vez, na ordem. Uma etapa só é concluída quando
todos os critérios de aceite passam. Ao concluir, marcar os checkboxes e registrar
aprendizados/decisões em `docs/DECISOES.md`.

Legenda: 🎯 = critério de aceite | 🦀 = conceito de Rust a dominar nesta etapa

---

## Etapa 1 — Fundação async ✅

- [x] `cargo new url-shortener` + dependências: tokio (full), axum
- [x] Rota `GET /health` retornando 200 com JSON
- [x] Struct `AppState` (vazio por ora) circulando via `State<Arc<AppState>>`
- [x] `tracing-subscriber` inicializado no main com `TraceLayer` do tower-http

🎯 `curl localhost:3000/health` responde e o log estruturado da request aparece.
🦀 Runtime Tokio, handlers async, extractors, por que não bloquear o runtime.

## Etapa 2 — Base62 + bijeção ofuscadora ✅

- [x] `codec/base62.rs`: `encode(u64) -> String` e `decode(&str) -> Option<u64>`
- [x] `codec/obfuscate.rs`: bijeção sobre u64 (multiplicação modular por primo
      com inverso pré-calculado, OU rede de Feistel de 3-4 rounds)
- [x] Testes: roundtrip encode/decode, roundtrip obfuscate/deobfuscate,
      rejeição de caracteres inválidos, casos extremos (0, u64::MAX no domínio)

🎯 `cargo test` verde; dois IDs consecutivos geram códigos visualmente não relacionados.
🦀 Módulos, `Option`/`Result`, `#[cfg(test)]`, aritmética com `wrapping_*`/`checked_*`.

## Etapa 3 — Persistência com sqlx ✅

- [x] Postgres local via docker-compose (`infra/docker-compose.yml`)
- [x] Migration: tabela `urls` (id bigserial PK, url_hash unique gerado, long_url,
      created_at). **Sem `short_code`**: o código é função pura do id, guardá-lo
      seria dado derivado que pode divergir — ver `docs/DECISOES.md`.
- [x] `PgPool` no `AppState`, criado no main com `max_connections` vindo de config
- [x] `repo.rs`: inserir com `ON CONFLICT DO NOTHING RETURNING id` + SELECT fallback
      (idempotência no banco, não na aplicação)
- [x] `sqlx::query_scalar!` compilando contra o schema real (+ `.sqlx/` para build offline)

🎯 Inserir a mesma URL duas vezes retorna o MESMO id, provado por teste de integração.
🦀 async com banco, macros do sqlx, `DATABASE_URL` em `.env`, `Result` + `?`.

## Etapa 4 — Rotas de escrita e leitura ✅

- [x] `error.rs`: enum `AppError` (NotFound, InvalidUrl, InvalidBody, Database, Internal)
      com `impl IntoResponse`. Corpo segue **RFC 9457** (`application/problem+json`).
- [x] `POST /v1/shorten`: `Json<ShortenRequest>`, validação http/https (crate `url`),
      fluxo id → obfuscate → base62 → responder
- [x] `GET /{code}` — **não** `/v1/urls/{code}`: o prefixo gastaria 9 chars numa URL
      cujo objetivo é ser curta. 301 montado à mão (`Redirect::permanent` emite 308).
- [x] 404 limpo para código inexistente **e para código malformado** (separar os dois
      vazaria o formato do shortcode), 400 para URL inválida

🎯 Fluxo completo via curl: encurtar → seguir redirect → chegar na URL original.
🦀 `Deserialize`/`Serialize` com serde, `?` propagando para `AppError`, `IntoResponse`.

## Etapa 5 — Cache Redis (cache-aside + cache negativo) ✅

- [x] Redis no docker-compose com `maxmemory` definido e `allkeys-lru`
- [x] Cliente Redis no `AppState` (crate `redis` 1.x, não `fred` — ver `docs/DECISOES.md`)
- [x] `cache.rs`: no GET, tentar cache → miss → banco → gravar no cache SEMPRE (sem TTL)
- [x] Popular o cache também na escrita (o dado acabou de nascer quente)
- [x] Cache negativo: código inexistente grava sentinela com **SET NX + TTL curto**
- [x] Contadores de hit/miss (`tracing` com campo `cache=hit|miss|hit_negative`)
- [x] **Extra:** rate limit por IP no `POST /v1/shorten` (`tower_governor`).
      O redirect fica sem limite de propósito — é o caminho que o cache absorve
      e que as Etapas 9-10 empurram a 11k req/s.

🎯 Segunda leitura do mesmo código não toca o Postgres (provar por log/métrica).
🎯 Teste da corrida: escrita concorrente positiva nunca é sobrescrita por negativa.
🦀 Traits de cliente async, serialização para o cache, TTL condicional.

## Etapa 5.1 — "Minhas URLs" sem login (só front) ✅

- [x] `localStorage` guarda os códigos criados neste browser
- [x] Listar com destino e link curto; ação de **remover da lista**
- [x] Deixar explícito na UI: salvo só neste navegador, e remover não desativa o link
- [x] **Extra:** redesign do front — medidor de compressão, paleta ferro/óxido,
      JetBrains Mono self-hosted subsetada (5 KB por peso), botão de copiar

🎯 Fechar o browser e voltar mantém a lista; limpar dados do site zera.
🦀 Persistência no browser via `web-sys`, estado derivado com signals do Leptos.

**Por que esta etapa existe:** encurtar e perder o código é o único jeito de o produto
falhar sem dar erro. A necessidade apareceu com o site no ar — sem lista, quem fecha a
aba perde o link, e não há nenhuma forma de recuperá-lo (a busca é por código, nunca
por URL longa).

**Por que não tem back:** o código é imutável, e é isso que sustenta o cache sem TTL
(Etapa 5) e o 301. Exclusão real exigiria invalidar o Redis e trocar 301 por 302 —
e nem assim funcionaria, porque um 301 já cacheado no browser redireciona para sempre.
Além disso a idempotência é global: o mesmo código pode ter sido criado por várias
pessoas, então apagar seria apagar o link de outro. Ver `docs/DECISOES.md`.

A Etapa 12 troca o 301 por 302 **só nas URLs com dono** — o que não contradiz o
parágrafo acima: continua não havendo exclusão, e o 302 existe para contar clique, não
para permitir desativar link.

Precursor das Etapas 11 e 12: a lista por browser vira lista por conta quando houver
login. Nada aqui vira dívida — a lista local continua valendo para quem não se cadastrar.

## Etapa 5.2 — Acertos do Lighthouse (só o que é barato)

Linha de base de 2026-07-26 e a análise completa em `docs/PERFORMANCE-WEB.md`:
desempenho 99 (mobile) / 100 (desktop), acessibilidade **91**, TBT 0, CLS 0.
Performance já está no teto — o que sobra é acessibilidade e segurança.

- [x] **Contraste**: resolvido no redesign da 5.1. Os tokens viraram três papéis
      (`--accent-face`, `--accent-ink`, `--accent-text`) em vez de um `--accent`
      servindo fundo claro e escuro, que era a raiz do problema
- [x] `<label>` oculto no input (havia só `placeholder`, que some ao digitar) e
      `aria-live="polite"` no resultado, senão o leitor de tela não anuncia o link gerado
- [x] `100svh` no lugar de `100vh`; alvo de toque do botão em 44 px; `flex-wrap` no
      formulário abaixo de ~420 px; `overflow-wrap: anywhere` no lugar de `break-all`
- [x] CSS inline via `data-trunk rel="inline"` — elimina os 150 ms de bloqueio de
      renderização, ao custo de o CSS viajar dentro do HTML `no-cache`
- [x] **Extra:** `preload` das duas fontes. Como o `<body>` vai vazio, nada pedia por elas
      até o wasm montar a UI — exatamente o instante em que a página tem o que mostrar
- [x] Headers de segurança no Traefik: HSTS, `frame-ancestors`, COOP, nosniff,
      `Referrer-Policy` — **aplicados em 2026-07-26**, verificados pela Cloudflare e direto
      na origem

🎯 ✅ **Acessibilidade 91 → 100** confirmado em 2026-07-26, com desempenho 98, práticas
   recomendadas 100, SEO 100 e navegação agêntica 2/2.

**Sobre "chegar a 100 em desempenho":** 98 e 99 são a mesma medição com ruído — o índice
oscila entre execuções do mesmo build. O que dava para atacar objetivamente era o bloqueio
de renderização, e ele saiu. O que sobra é o script que a Cloudflare injeta e o tamanho do
wasm (reativar `wasm-opt` quando o binaryen arm64 funcionar).

**Fora desta etapa, de propósito:** CSP (o trunk emite o bootstrap do wasm como script
inline, então exige hash por build ou nonce — trabalho de verdade) e markup estático/SSR
para melhorar FCP. Os dois estão em `docs/PERFORMANCE-WEB.md` com o custo estimado.

**Não é problema nosso:** os avisos de cache de 5 KiB e parte dos "3 KiB de JS" vêm do
script que a Cloudflare injeta no `<body>` (`max-age=300`, verificado em produção).
Some desligando *Bot Fight Mode* — decisão de segurança, não de performance.

## Etapa 5.3 — Idioma pelo navegador ✅

- [x] Detectar por `navigator.language` (via `web-sys`), **com pt-BR como padrão**
- [x] **Seletor visível**, e não só detecção — quem usa o sistema em inglês e lê português
      fica preso sem ele. A escolha explícita grava em `localStorage`
      (`oxid.locale.v1`, ao lado da lista) e vence o navegador
- [x] `document.documentElement.lang` corrigido no mount — é esse atributo que o leitor de
      tela usa para escolher a pronúncia
- [x] `<title>` e `<meta description>` acompanham o idioma escolhido
- [x] Todas as strings num catálogo só, nenhuma literal solta no `view!`

🎯 ✅ Abrir com o navegador em pt-BR mostra a interface em português; trocar no seletor
   sobrevive ao reload.
🦀 `navigator.language` via web-sys, catálogo `&'static`, sinal de locale.

**Decisão — `match`, sem crate.** `leptos_i18n` traz Fluent, plural e interpolação; com dois
idiomas e 19 strings, um `enum Locale` com catálogo `&'static Strings` resolve sem
dependência nenhuma. A conta vira quando aparecer plural de verdade ou formatação de data.

**Decisão — o `index.html` declara `lang="pt-BR"`.** O documento estático é o que o crawler
lê e o que pinta antes do wasm montar. Declarar `en` e trocar depois significaria anunciar o
idioma errado ao leitor de tela por todo o tempo de carga do bundle.

**Ficou como está — o erro da API continua em inglês.** O `detail` da RFC 9457 é gerado pelo
servidor. Traduzir no front, casando por `title`, duplicaria o catálogo e serviria só a este
cliente; o certo é negociar `Accept-Language` na API, e isso é i18n no back — trabalho
próprio, não um apêndice desta etapa.

**O ponto não óbvio — o erro vem do servidor em inglês.** O `detail` da RFC 9457 é gerado
pela API. Duas saídas, e elas não são equivalentes:

- **Traduzir no front, mapeando por `title`/`type`.** O contrato já promete que esses campos
  são estáveis justamente para o cliente casar em cima deles. Não toca no back, mas duplica
  catálogo e só serve a este front.
- **Negociar no back por `Accept-Language`.** Mais correto: quem chama a API por `curl` ou
  script recebe o erro no idioma pedido. É o único caminho se um dia houver outro cliente —
  e implica i18n no back também.

**Limite conhecido:** com CSR, o crawler recebe o HTML estático em inglês qualquer que seja o
leitor. `hreflang` e título traduzido só valem de verdade com SSR, que está fora de escopo.

## Etapa 6 — Configuração e dimensionamento ✅

- [x] Toda config fora do código: `base.yaml` → `<ambiente>.yaml` → `APP_*`
      (YAML em vez de `.env`; ver `docs/DECISOES.md`, Etapa 3, decisão 6)
- [x] `.env.example` documentado
- [x] Pool do Postgres pequeno por padrão — `max_connections: 8`
- [x] Timeouts de acquire do pool (3 s) e de conexão do Redis (2 s)
- [x] **`statement_timeout` (3 s)** via `PgConnectOptions::options`, aplicado por conexão em
      vez de depender de config do servidor — o mesmo banco pode servir uma migration ou uma
      sessão manual que legitimamente demore mais
- [ ] Reavaliar `idle_timeout` e `max_lifetime` do pool — só faz sentido com número na mão,
      depois da Etapa 9

🎯 ✅ App sobe em ambiente limpo só com o YAML preenchido; pool visível nas métricas.
🦀 Structs de config com serde, `Duration`, fail-fast no bootstrap.

**Por que o `statement_timeout` importa mais aqui do que parece:** com pool grande, uma query
lenta degrada; com pool de 8, ela **esgota**. É o tipo de falha que só aparece sob carga —
ou seja, na Etapa 9, quando o custo de descobrir é bem maior.

## Etapa 7 — Observabilidade

- [x] `metrics` + `metrics-exporter-prometheus`, endpoint `/metrics` **em porta própria**
- [x] Histograma de latência por rota, contador `cache_lookups_total` por desfecho,
      gauges do pool
- [x] Prometheus + Grafana em `infra/docker-compose.observability.yml`
- [x] Dashboard provisionado por arquivo: p50/p95/p99 por rota, req/s por status, hit rate,
      lookups por desfecho, conexões do pool
- [x] Prometheus no cluster raspando os pods, por `PodMonitor` — as duas réplicas com
      `up=1`, e o Grafana do cluster lendo daí, não só o compose local
- [ ] Exporters de Postgres e Redis. Os dois são caixa-preta hoje, e a Etapa 10 vai
      precisar decidir entre "o banco está lento" e "o pool está cheio" — perguntas
      diferentes que a métrica da aplicação não separa
- [ ] Métricas do **proxy**, no lugar do exporter de Nginx que esta etapa previa. Não
      há Nginx balanceador (ver Etapa 8); quem está no caminho de todo tráfego é o
      Traefik, e ele expõe Prometheus nativamente

🎯 Dá para responder "onde está o gargalo?" olhando um único dashboard.
🦀 Macros de métricas, custo de instrumentar, cardinalidade de labels.

**`/metrics` não é rota do router público.** O Traefik encaminha para a API tudo que não é
o front, então uma rota `/metrics` estaria legível na internet — entregando volume de
requisições, distribuição de latência e comportamento do cache a quem pedisse. Vai num
listener separado (9090), declarado no Deployment e **ausente do Service**.

**O label de rota vem do `MatchedPath`, não do path.** Em `/{code}` os dois diferem por
construção: o path real é uma string diferente a cada request, então rotular com ele criaria
uma série temporal por shortcode e derrubaria o Prometheus muito antes do serviço. É a única
linha do middleware que não pode estar errada.

**Buckets escolhidos, não os padrão.** O conjunto default se espalha por uma faixa que este
serviço nunca usa. A meta da Etapa 10 é p95 < 50 ms e um hit de cache responde em
milissegundos de um dígito — a resolução tem que estar embaixo de 100 ms, que é onde as
respostas caem.

**Gauges do pool amostrados no scrape**, não por task com timer: lidos na hora do scrape
nunca estão mais velhos que o próprio scrape, e não há um segundo relógio para raciocinar.
`total - idle` é o número que importa; grudado em `max_connections` significa fila no pool,
não no banco.

## Etapa 8 — O teto do nó único

Reescrita em 2026-07-28. A versão original foi escrita antes do k3s existir, e metade
dela aconteceu por outro caminho: "2 instâncias atrás de Nginx" hoje são 2 pods atrás
de um Service, quem balanceia é o kube-proxy, e o Nginx do projeto serve só os
estáticos do front — não há `upstream` nem `proxy_pass` em `infra/nginx/web.conf`. O
build multi-stage em `--release` já está no `infra/Dockerfile` desde o primeiro deploy.

O que sobrou de real virou o tema: **conhecer o teto do que já existe e deixar a
medição da Etapa 9 acontecer sem ruído.**

- [x] Levantar sysctls e limites de file descriptor nas três camadas, com a coluna que
      faltava: **quais são por network namespace**. Host, container do proxy e pod não
      compartilham os de rede, e um namespace novo nasce com o default do kernel, não
      com o valor atual do host. Ajustar no lugar errado não faz nada e parece que fez
- [x] Limites de file descriptor: verificar antes de ajustar. Runtimes de container
      costumam subir com limites altos e os processos herdam — o `ulimit -n` baixo que
      aparece numa sessão SSH é do shell de login e não vale para serviço nenhum
- [x] A conta do pool: réplicas × `max_connections` contra o **paralelismo útil** do
      banco, que é menor que o número de cores. O número final sai da Etapa 10
- [x] `requests` e `limits` revisados — a conclusão foi não mudar antes de medir
- [x] Quanto do nó é do serviço e quanto é de vizinho
- [x] Réplicas: **ficam 2, pela continuidade em crash**, com o motivo escrito por
      extenso — não somam throughput e não são necessárias para rolling update
- [x] Decidido o que fazer com o peso de cgroup da árvore do Kubernetes: **nada, e
      medir como está** — ver abaixo

🎯 A Etapa 9 pode medir sabendo o que está medindo: linha de base registrada, cada
   parâmetro com sua camada e o sintoma que justificaria mexer nele, e nenhuma mudança
   feita às cegas.

**Os números medidos ficam fora do git**, em `docs/`. São a medição de **um** host, com
a capacidade e os vizinhos dele; para quem clona o repositório, baseline alheio não é
dado, é ruído. O que fica aqui é o método e o que ele revelou de generalizável.

**Nada foi ajustado, de propósito.** O método das Etapas 9 e 10 é hipótese → medição →
**uma** mudança → confirmação. Vários parâmetros mexidos antes da primeira medição
destroem a capacidade de atribuir qualquer melhora a qualquer causa, e todo ajuste feito
às cegas vira "sempre foi assim" na leitura seguinte. Cada candidato fica registrado ao
lado do sintoma que o justificaria, e não antes dele.

**O achado que muda como ler a Etapa 9: as `requests` do Kubernetes não valem contra
quem está fora dele.** Elas ordenam a disputa *dentro* da árvore de cgroup do cluster.
Se o mesmo host roda containers por fora — um proxy, outros projetos, qualquer coisa em
Docker —, a disputa entre as duas árvores é decidida um nível acima, onde o kubelet não
manda.

E o peso que ele coloca lá tende a ser **menor** que o padrão de qualquer serviço do
sistema. O kubelet expressa a capacidade do nó na escala antiga de *shares*; a conversão
para a escala do cgroup v2 comprime esse valor bem abaixo do 100 com que nasce uma unit
comum do systemd. Não é política, é perda de impedância entre duas escalas.

Isso convive com o `Allocatable` do nó, que é o que o scheduler usa e está correto
dentro do mundo dele. Os dois números discordam, e é o do cgroup que a carga encontra.
Sem isso registrado, o teto da Etapa 9 seria lido como "o teto do serviço" quando é o
teto de uma fatia que ninguém dimensionou de propósito.

Vale para qualquer cluster que divida o host com Docker — que é o caso de toda máquina
onde k3s convive com um proxy gerenciado por fora.

**Decidido: medir como está.** As opções eram três — deixar quieto, subir o peso da
árvore do cluster, ou baixar o de quem divide o host com ela. Ficou a primeira, por dois
motivos. O peso não é defeito do instrumento, é a condição em que o serviço de fato
roda: corrigi-lo antes seria medir um sistema que não existe. E há chance concreta de a
CPU não saturar, o que encerra a discussão sem custo nenhum. Se saturar e a fatia for o
limite, isso vira a **primeira iteração da Etapa 10**, com antes e depois medidos — que
é onde está o aprendizado.

**Quando for mexer, mexa do lado de fora.** Alterar o `cpu.weight` da árvore do cluster
diretamente não gruda: o kubelet reconcilia e a mudança some, o mesmo problema do
`iptables` reescrito pelo k3s. O ponto estável é o outro lado da disputa —
`systemctl set-property` no serviço que divide o host, que ninguém reconcilia por cima.

**Duas réplicas no mesmo nó não somam throughput.** Um único pod já enxerga todos os
cores do nó e os usa — o runtime do Tokio abre um worker por core visível. Dois pods são
dois runtimes disputando os mesmos cores. O que elas dão é **continuidade durante
crash**: um panic não derruba o serviço. E note que rolling update não é argumento — com
`maxUnavailable: 0` e `maxSurge: 1`, uma réplica única também sobe a nova antes de matar
a velha.

**O custo das duas réplicas está no pool, não na memória.** Um pod ocioso consome
memória desprezível, mas `max_connections` vale **por processo**: duas réplicas dobram o
teto de conexões contra o mesmo banco. Conexões ociosas não custam; conexões *ativas*
além do que o banco executa em paralelo não viram throughput, viram troca de contexto.

**A regra "cores do banco × 2" esconde o que importa.** O pool não precisa caber no
número de cores, precisa caber no *paralelismo útil*. Pela lei de Little, o pool
necessário é `taxa × tempo de serviço` — com escritas de poucos milissegundos e um cache
que absorve a leitura, isso dá um número de um dígito, não dezenas. Os dois erros têm o
mesmo relógio e por isso se confundem: pool pequeno demais **enfileira**, e a espera só
estoura no timeout de acquire, muito depois de o p95 ir embora; pool grande demais
**esconde** a saturação do banco até virar timeout de statement. É por isso que o gauge
de conexões do pool (Etapa 7) vale mais aqui do que a latência — colado no máximo
significa fila no pool, longe dele com latência alta significa problema no banco.

**O critério 50/50 foi descartado, e não só por causa do kube-proxy.** É verdade que
ele sorteia por conexão e não por requisição, então com keep-alive uma conexão sorteada
carrega centenas de requisições atrás dela e o desvio se amplifica — foi o que a
medição de tráfego real mostrou. Mas o problema mais fundo é outro: **simetria entre
dois processos na mesma CPU não significa nada.** Não há isolamento de falha de máquina,
não há soma de capacidade, não há nada que o 50/50 garanta. Perseguir esse número seria
otimizar uma métrica sem consequência.

**O nó pode não ser só do serviço, e isso muda o que a medição significa.** Se o host
serve outras coisas — outros projetos, o proxy, a própria observabilidade —, o teto que
a Etapa 9 encontra é o teto **daquele** nó como ele está, não o do código. Saber quanto
da capacidade é vizinho é pré-requisito para atribuir o gargalo a alguém. Antes de
medir, contabilizar; depois de medir, subtrair.

**O observador entra na conta do observado.** Vale conferir quanto o próprio Prometheus
consome: numa máquina pequena ele compete de igual para igual com o que está sendo
medido, e sob carga raspa séries mais caras justamente quando a CPU é o recurso
disputado. Medir o custo dele **durante** o teste, não só depois.

## Etapa 9 — Teste de carga com k6

- [x] `infra/k6/load.js`: executor `ramping-arrival-rate`, rampa de 30s,
      proporção 1:10 escrita/leitura, pool de URLs pré-criadas para as leituras
- [x] Thresholds no script: p95 alvo, taxa de erro 0 e `dropped_iterations = 0`
- [x] Checklist de validade do teste em `infra/k6/README.md`
- [x] Rodar em escala 0.5 — e depois varrer para baixo, porque ela satura

🎯 ✅ **Pelas duas vias.** Há um run com percentis limpos e zero erros — numa escala
   menor que a pedida — e o gargalo da escala 0.5 está identificado com telemetria dos
   dois lados. O gargalo é CPU: não é banco (pool ocioso), não é cache (100% de hit),
   não é rede (os dois lados convergem sob carga). Números em `docs/ETAPA-9-CARGA.md`.

**A escala 0.5 não passa, e varrer para baixo é que deu o número útil.** O critério
pedia a escala 0.5; ela satura. O que fecha a etapa é a maior escala com percentis
limpos, encontrada descendo até o run em que `dropped_iterations` zera e o p95 entra na
meta. "Falhou na escala pedida" e "não se sabe a capacidade" são coisas diferentes, e só
a varredura separa as duas.

**O joelho é estreito, e é a assinatura de fila.** Entre a maior escala limpa e a
seguinte, o p95 saltou quase 5x para 1,5x de carga. Não é degradação suave: é uma fila
cruzando o ponto de saturação. Perto do joelho, portanto, capacidade extra compra muito
pouca latência — e é por isso que a Etapa 10 tem de atacar o recurso saturado, não
espremer configuração.

**Zero erros em toda escala testada.** Nenhum 5xx, nenhuma conexão recusada, nem na
escala que saturou o nó. O serviço não quebra sob carga — ele enfileira, e a latência
sobe. Vale registrar porque era um requisito do projeto ("zero erros sob carga de
pico") e ele passou mesmo onde a latência não passou.

**O seed já respondeu a pergunta da escrita.** Criar o pool de leitura é, ele próprio,
um teste de escrita — e sustentou acima da meta da escala 1.0, com zero falhas, antes
de o teste formal começar. Quando um passo preparatório mede algo, vale ler o número.

**A rede deixou de importar exatamente quando passou a haver fila.** Medindo de fora
do datacentre, em carga baixa o RTT era quase toda a latência; sob saturação, a fila do
servidor domina tanto que a rede vira ruído. A consequência prática é boa: **não é
preciso um gerador dentro do datacentre para achar o joelho** — a diferença entre os
dois lados encolhe justamente na região que interessa.

**A convergência dos dois lados é o que torna o run defensável.** A telemetria do
servidor não depende do gerador. Quando ela confirma o cliente, um `dropped_iterations`
diferente de zero deixa de invalidar a conclusão — mede-se o erro do gerador, não o do
alvo. Sem os dois lados, o run inteiro seria descartável pelo próprio checklist.

**O erro de método que este teste cometeu, e que vale evitar:** dimensionar os VUs do
gerador exige conhecer a latência, que é justamente o que o teste vai descobrir. A
estimativa inicial errou por 4x, o pool de VUs estourou e as iterações começaram a ser
descartadas. A saída é iterar — rodar, ler a latência, redimensionar — ou pré-alocar com
folga larga desde o começo. É a lei de Little de novo, a mesma do pool de conexões.

**Distribuição diz o tipo de problema, média não diz nada.** Mediana baixa com p95 alto
é fila **intermitente**; um serviço uniformemente lento teria a mediana alta também. Ler
o par mediana/p95 antes de formular qualquer hipótese economiza uma iteração inteira.

## Etapa 10 — Ciclo de otimização até escala 1.0

- [ ] Método fixo: hipótese → medição → mudança (UMA por vez) → confirmação
- [ ] Registrar cada iteração em `docs/DECISOES.md` (o que media, o que mudou, resultado)
- [ ] Subir escala: 0.5 → 0.75 → 1.0 (≈ 11.574 leituras/s + 1.157 escritas/s)
- [ ] Suspeitos prováveis, nesta ordem: kernel do host (portas efêmeras, `conntrack`,
      sockets), pool do Postgres, contenção de CPU com os vizinhos do nó, serialização
      no hot path, limites de file descriptors

🎯 Escala 1.0 sustentada com zero erros e p95 de leitura < 50ms.

---

## Etapa 11 — Contas e sessão

Depois da Etapa 10, de propósito: autenticação não muda o perfil de carga do sistema,
e as Etapas 9-10 medem melhor um sistema sem sessão no caminho.

- [x] Migration `users` (id, email `citext` unique, `password_hash`, created_at)
- [x] Migration `short_codes` — **substitui `url_owners`**, ver a decisão revista abaixo
- [x] Hash argon2id; verificação sempre em tempo constante, inclusive para e-mail
      inexistente (hash falso), senão o tempo de resposta vira oráculo de cadastro
- [x] Sessão no Redis, cookie `HttpOnly` + `Secure` + `SameSite=Lax`, id de 128 bits
- [x] `POST /v1/signup`, `POST /v1/login`, `POST /v1/logout`, `GET /v1/me`
- [x] `GET /v1/urls` — lista do dono, paginada por keyset (não OFFSET)
- [x] `POST /v1/shorten` associa ao dono quando há sessão; sem sessão segue igual
- [x] Rate limit próprio nas rotas caras, separado do de `shorten`
- [x] **Extra:** hashagem fora do runtime e sob teto de concorrência — ver abaixo
- [x] **Extra:** front com diálogo de conta, lista da conta e importação no cadastro
- [x] **Extra:** `POST /v1/logout-all` — "sair de todos os dispositivos", ver abaixo
- [x] **Extra:** testes de CORS/CSRF travando a proteção emergente, ver abaixo
- [ ] Fechar a enumeração de contas no `signup` — precisa de e-mail, ver abaixo

🎯 ✅ Duas contas encurtando a mesma URL recebem **códigos diferentes** apontando para a
   **mesma linha** em `urls`; a mesma conta encurtando duas vezes recebe o mesmo código;
   e o código anônimo criado antes continua resolvendo, inalterado.
🦀 Extractor de auth no Axum, `argon2`, cookies, keyset pagination, `spawn_blocking`.

**Decisão revista (2026-07-29): o código é por dono, a URL continua deduplicada.**
O ADR de 26/07 escolhia `url_owners` N:N com código global, e rejeitava o código por
usuário porque `UNIQUE (user_id, url_hash)` multiplicaria linhas num modelo que projeta
365 bilhões delas contando com o dedupe. A rejeição estava certa; a conclusão, não.

O problema é que `urls` fazia dois trabalhos com regras de unicidade diferentes: guardar
a URL longa e definir o código. Separando em `short_codes (id, url_id, owner_id)`, a URL
longa continua armazenada uma vez — o dedupe do dado **pesado** sobrevive — e o que
multiplica é uma linha de três bigints por (dono, URL).

O que isso resolve, e é o motivo de valer uma migration:

- **Cliques atribuíveis.** Um clique chega como `GET /{code}` e o código é a única
  informação disponível. Com código compartilhado, um clique pertence a todos os donos ao
  mesmo tempo e nada no request permite escolher. Com código por dono, a atribuição é
  estrutural — sem tabela de rateio e sem regra a documentar.
- **O 302 deixa de contaminar o link anônimo.** A Etapa 12 registrava como dano aceito
  que "um dono faz o código inteiro virar 302, inclusive para quem chegou pelo link
  anônimo". Agora o código sem dono continua 301 e cacheável, que é o caminho que as
  Etapas 9-10 medem.
- **O flag `owned` no cache deixa de existir** — com monotonicidade e instante de
  invalidação, três parágrafos de complexidade. O código já nasce sabendo se tem dono, e
  o cache volta a ser imutável sem ressalva.

O que se perde é "duas pessoas diferentes recebem o mesmo código", que não servia a
ninguém. A idempotência com valor prático — a mesma pessoa encurtando duas vezes — fica.

**`UNIQUE NULLS NOT DISTINCT` é a linha que sustenta isso.** Uma `UNIQUE` comum trata
`NULL` como nunca igual a `NULL`, então permitiria dois códigos anônimos para a mesma
URL — quebrando em silêncio a idempotência que já funcionava. Nada erra, duplicatas só
acumulam. Tem teste próprio, porque é o tipo de falha que passa em todos os outros.

**Argon2 bloqueia, e isso era pior que o rate limit furado.** A verificação é síncrona e
CPU-bound: chamada direto de um handler `async`, ocupa um worker do Tokio por dezenas de
milissegundos. Num nó de dois cores há dois workers, então duas tentativas simultâneas
paravam o runtime inteiro — redirect incluído. Um atacante não precisava saturar CPU,
precisava de duas conexões. Resolvido com `spawn_blocking`.

**E um teto de concorrência que não depende de identificar ninguém.** O limite por IP
protege os endpoints caros, mas já falhou em silêncio atrás da CDN uma vez. Um semáforo
em volta da hashagem limita o Argon2 em voo independentemente de quem chama: uma enxurrada
vira fila, e além do teto a resposta é 503 com `Retry-After`. O caminho do decoy também
consome slot — isentá-lo daria a volta no teto usando só e-mails inexistentes, que é o
caminho mais barato de descobrir.

**"Sair de todos os dispositivos" (2026-07-29), a partir da auditoria.** O `SessionStore`
só tinha `s:<id> → user_id` — busca em um sentido só, sem como listar as sessões de um
usuário. Isso tornava impossível revogar tudo num incidente de conta comprometida. A
correção é um índice reverso: ao criar uma sessão, ela também entra num conjunto
`u:<user_id>`; o `revoke_all` lê o conjunto, apaga cada `s:<id>` e por fim o próprio
conjunto. `POST /v1/logout-all` exige sessão válida (só revoga as próprias) e, ao contrário
do `logout` comum, **não engole falha** — quem clica está reagindo a um comprometimento, e
dizer "saiu" quando a revogação falhou seria a pior mentira. Namespace configurável no
store para os testes ficarem isolados no Redis compartilhado.

**CORS/CSRF era proteção emergente; virou proteção testada (2026-07-29).** Não há token
CSRF nem `CorsLayer`, e isso é a postura segura, não uma lacuna: sem CORS é mesma-origem
apenas, e as rotas de mutação exigem `Json<T>`, que um `<form>` cross-site não produz. O
risco era isso sumir no dia que alguém adicionasse um `CorsLayer` permissivo sem refazer o
raciocínio. Agora o CI é dono dessa garantia: testes que falham se um preflight cross-origin
receber `allow-origin`, se um POST `form-urlencoded` numa rota de mutação for aceito, ou se o
cookie de sessão perder `HttpOnly`/`Secure`/`SameSite=Lax`. Quando o CORS finalmente for
preciso (a extensão da Etapa 13), tem de ser **escopado**, nunca permissivo — e o teste
obriga a decisão a ser consciente.

**Pendência — o `signup` entrega a enumeração que o `login` protege.** O login responde
igual nos dois casos, com o mesmo `type` e o mesmo custo de CPU. O signup responde 409
quando o e-mail existe, então basta tentar cadastrar para saber quem tem conta.

Fechar isso exige e-mail: responder 200, não criar nada e mandar mensagem ao endereço —
quem é dono descobre, quem sonda não. Sem caminho de e-mail, a alternativa seria responder
200 mentindo, e deixar sem explicação quem digitou um endereço já cadastrado. Fica
registrado como escolha, não como esquecimento.

**Não há exclusão de conta, e não é omissão.** `short_codes.owner_id` referencia `users`,
então a FK barra o `DELETE`; um `ON DELETE SET NULL` transformaria os links da pessoa em
links anônimos. Qual das duas é a resposta certa é decisão de produto, e ela precisa ser
tomada **antes** da Etapa 12 — analytics com dono que pode desaparecer é pergunta sem
resposta depois que existem dados.

## Etapa 12 — Analytics de clique

**Decisão (2026-07-29): ClickHouse, não Postgres, e TTL de 30 dias.** O roadmap sugeria
`postgres` como default e ClickHouse opcional; a escolha foi ClickHouse direto, porque é a
ferramenta feita para `count`/`uniq` sobre janela de tempo, e num projeto de aprender
system design o paradigma colunar vale por si. O TTL caiu para **30 dias** — uma janela
rolante, não o arquivo de 10 anos que dimensiona o resto do sistema. Duas consequências:

- **Cai a projeção de bilhões de linhas.** 30 dias de clique é pequeno, e a pressão de
  memória no nó de 2 vCPU deixa de ser problema (com `mem_limit` no container, obrigatório).
- **A cadeia de FK some.** ClickHouse não referencia o `users`/`short_codes` do Postgres,
  então excluir conta **não** cascateia para os cliques — vira `ALTER TABLE ... DELETE`
  explícito. Registrado como decisão, não esquecimento. Isso *substitui* o pré-requisito
  de `ON DELETE` que a Etapa 11 travava: com backend desacoplado, não há cascata a decidir.

**Duas telas de análise** (2026-07-29): uma **geral** (série de cliques por dia com uma
linha por código do dono) e uma **individual** por link (a mesma série de um código só, com
range 7/14/21/28d). Ambas saem do mesmo `summary()` — a única diferença é o filtro por
`code_id`, e o `ORDER BY (code_id, created_at)` faz as duas leituras serem rápidas.

- [x] **Fatia 1 — o sink, inerte.** Tabela `click_events` (`MergeTree`, `PARTITION BY
      toYYYYMM`, `ORDER BY (code_id, created_at)`, `TTL 30 DAY`), o enum `ClickSink`
      (`Disabled`/`ClickHouse`) com `record` (insert em lote) e `summary` (`count` +
      `uniq(visitor_hash)` + série `toStartOfDay`) implementados e testados contra o
      ClickHouse real. ClickHouse no compose com memória limitada; config `analytics.backend
      = off | clickhouse`, `off` por default. Nada plugado no hot path ainda
- [x] **Fatia 2 — o pipeline:** `mpsc` → worker → `record` em lote. `try_send` que descarta
      e conta o descarte se encher, nunca bloqueia. Testado nos dois momentos em que um
      lote é escrito sem encher: o timer, e o desligamento — este é o que perde clique
      em silêncio se faltar
- [x] `302` quando o código tem dono, `301` quando não tem — o caminho anônimo
      continua cacheável pelo browser e é o que o k6 exercita
- [x] ~~Flag `owned` no valor cacheado~~ — **desnecessário** desde a Etapa 11: o código
      é por dono, então `short_codes.owner_id` já responde isso sem tocar o cache
- [x] `click_events` referencia **`short_codes.id`**, não `urls.id` — é a linha que faz
      `count(*) WHERE code_id = $1` ser a métrica de uma pessoa em vez de uma soma ambígua
- [x] Evento por clique sai do hot path: `mpsc` → worker → insert em lote
- [x] ~~**Os dois destinos no código**: `analytics.backend = postgres | clickhouse | off`~~
      — **descartado** pela decisão do topo desta etapa, que escolheu ClickHouse direto.
      Ficou `off | clickhouse`. O interruptor que importava era o `off`, para a analytics
      não contaminar a medição das Etapas 9-10, e esse existe
- [ ] Captura: ts, url_id, país (`CF-IPCountry`), browser/OS/dispositivo do user-agent,
      host do referer, idioma, flag de bot. **As colunas existem no schema desde a Fatia 1
      e gravam string vazia** — a tabela já está particionada, então preenchê-las depois não
      exige `ALTER`. É o que falta desta etapa, junto do item abaixo que depende dele
- [x] Visitante único sem cookie: `hash(ip + user-agent + salt do dia)` — o salt diário
      é o que impede reidentificar alguém entre dias. **Só passou a funcionar em
      2026-07-31**: implementado desde o início, mas em produção contava ~1 visitante por
      clique, porque o Traefik descartava o `X-Forwarded-For` da Cloudflare. Ver a pendência
      de rate limit abaixo — era o mesmo header, e um ajuste resolveu os dois
- [ ] Dashboard: ~~cliques totais e únicos~~, ~~série temporal~~, top países, top
      referrers, dispositivos. **Metade pronta**: as duas telas (geral, com uma linha por
      código, e individual com janelas de 7/14/21/28d) mostram total, únicos e a série
      diária densa, com valor exato no hover. As três listas de topo dependem do
      enriquecimento acima

🎯 Um clique aparece no dashboard sem que o p95 do redirect se mova.
   **Metade verificada**: o clique aparece — comprovado em produção em 2026-07-30, com o
   pipeline inteiro (302 → emit → lote → ClickHouse → dashboard). O p95 **não foi medido
   de novo** desde que a analytics entrou no ar, então a segunda metade da meta continua
   sendo afirmação, não número. Fechar isso é repetir a corrida da Etapa 9 com
   `analytics.backend = clickhouse` e comparar com a linha de base em `off`.
🦀 Canal `mpsc` com backpressure, task de background, batch de escrita.

### Onde gravar o evento — as duas implementações, trocadas por config

Não escolher no papel: implementar as duas atrás do mesmo tipo e alternar por
configuração, do jeito que `Cache::disabled()` (Etapa 5) já faz. Assim os dois medem o
**mesmo tráfego** em vez de dois testes que não se comparam — que é o método fixo da
Etapa 10 aplicado a uma decisão de banco.

```rust
// Enum, não `Box<dyn Trait>`: são três variantes conhecidas em tempo de compilação,
// e `dyn` com `async fn` ainda exigiria `#[async_trait]` e uma alocação por chamada.
enum ClickSink {
    Disabled,
    Postgres(PgPool),
    ClickHouse(ClickHouseClient),
}

impl ClickSink {
    async fn record(&self, batch: &[ClickEvent]) -> Result<(), SinkError> { todo!() }
    async fn summary(&self, url_id: i64, range: DateRange) -> Result<Summary, SinkError> { todo!() }
}
```

**Onde essa abstração é barata e onde ela dói.** A escrita abstrai bem: `record` recebe
um lote e devolve `Result` — as duas implementações fazem literalmente isso. A leitura
**não** abstrai: as consultas do dashboard são dialetos diferentes (`date_trunc` +
`GROUP BY` contra `toStartOfDay` e funções de agregação do ClickHouse), então são dois
conjuntos de query devolvendo o mesmo `Summary`. É aí que mora o custo de manter as
duas, e é o que a assinatura acima deixa explícito ao separar `record` de `summary`.

**Contrapartida honesta:** duas implementações é o dobro de superfície para um projeto
que ainda não tem observabilidade (Etapa 7). O que paga é o interruptor permitir
responder "quanto o ClickHouse ganha aqui, de verdade?" com número em vez de opinião —
e desligar (`off`) durante as Etapas 9-10, para a analytics não contaminar a medição.

### Comparação para orientar o default

| | **A. Postgres particionado** | **B. ClickHouse** |
|---|---|---|
| Infra nova | nenhuma | mais um banco no mesmo nó |
| Escrita | `COPY`/insert em lote na tabela particionada por mês | `async_insert` ou lote de 10k+ |
| Agregação sobre milhões | índice ajuda até certo ponto; depois exige tabela de rollup | é o que ele faz de melhor |
| Retenção | `DROP PARTITION` | `TTL` na tabela |
| Custo de errar | baixo — dá para migrar depois | alto — tirar um banco do ar é pior que não colocar |
| Aprendizado | mais SQL e particionamento | um paradigma novo (colunar, merges, partes) |

**Default sugerido: `postgres`.** Enquanto o volume couber em "milhões por mês" e as
consultas forem as seis do dashboard, a tabela particionada resolve — e subir o
ClickHouse é opcional, não requisito para a etapa fechar. O `clickhouse` entra quando
aparecer consulta ad-hoc sobre o histórico inteiro ou ingestão que o Postgres não
sustente sem virar gargalo do redirect. Com o interruptor, essa virada é uma linha de
config e uma medição, não um rewrite.

O ADR de 2026-07-26 já dizia que a analytics de clique é o bom caso do ClickHouse; o
que ele não dizia é que "bom caso" e "vale o custo agora" são perguntas diferentes.

**Eram três coisas que esta etapa quebrava. A Etapa 11 dissolveu duas.**

1. **O 301 impede contar cliques** — o browser cacheia e o segundo clique nunca chega ao
   servidor. Por isso só o código com dono vira 302. Esta continua de pé: é da natureza
   do problema, não do modelo de dados.
2. ~~Um dono faz o código inteiro virar 302, inclusive para quem chegou pelo link
   anônimo.~~ **Resolvido.** Era consequência do código compartilhado; com código por
   dono, o anônimo continua 301 e cacheável — que é justamente o caminho que o k6 mede.
3. ~~O cache sem TTL supunha imutabilidade total, e o flag `owned` a quebrava.~~
   **Resolvido, e por não existir.** Sem flag no valor cacheado não há invalidação, não há
   monotonicidade a garantir e o cache volta a ser imutável sem ressalva.

O que **não** mudou: remover um link da lista não devolve o 301, e não poderia — um 301 já
cacheado no browser é irreversível.

**Restrição de infra que vale para as duas opções:** o alvo é um nó **pequeno e
único**, que já roda Postgres, Redis, as réplicas da API e o front. É esse orçamento —
e não a qualidade do banco — que faz a opção A começar na frente. Num cluster com nós
sobrando, a conta muda e o ClickHouse deixa de custar o que custa aqui.

## Etapa 13 — Extensão de navegador

Um clique no ícone encurta a página aberta e põe o link curto na área de transferência.
É onde o produto encosta no uso real: hoje encurtar exige sair da página, abrir o oxid,
copiar a URL e colar. A extensão apaga esses quatro passos.

**O um-clique exige estar logado.** É a decisão de produto desta etapa: sem conta, a
extensão não tem onde guardar o link — a lista por browser da Etapa 5.1 é do site, não da
extensão, e um link encurtado que some é o único jeito de o produto falhar sem dar erro
(Etapa 5.1). Com conta, o link cai na lista da pessoa e a extensão vira útil no dia a dia.
Depois da Etapa 11 de propósito, porque é ela que dá o "logado" em que isto se apoia.

- [ ] Manifest V3 (Chrome/Edge) e WebExtensions (Firefox) a partir do mesmo código
- [ ] Estado de login na extensão: sem credencial, o ícone abre "entre no oxid para usar",
      não um erro. Com credencial, é o um-clique
- [ ] Ação do ícone: URL da aba ativa → `POST /v1/shorten` autenticado → clipboard → badge
- [ ] Menu de contexto ("encurtar este link"), além da página atual
- [ ] Publicar nas duas lojas

🎯 Logado, um clique encurta a aba atual e copia o link, e ele aparece na lista da conta.
   Deslogado, o clique convida a entrar em vez de falhar.

**Permissão mínima é decisão de design, não detalhe.** `activeTab` dá acesso à aba **apenas
no clique**, e é tudo que esta extensão precisa. Pedir `host_permissions: ["<all_urls>"]` —
o caminho mais fácil — é pedir para ler qualquer página que a pessoa visite: atrasa a
revisão das lojas e transforma um comprometimento da extensão num vazamento do histórico
inteiro. `activeTab` + `clipboardWrite`, e nada mais.

**Duas coisas que precisam mudar no servidor:**

1. **CORS.** Hoje não existe, porque o front é mesma origem e nunca precisou. A extensão
   chama de `chrome-extension://…`, que é outra origem. Ou a API ganha um `CorsLayer` em
   `/v1/shorten`, ou a extensão declara `host_permissions` para `oxid.uk` e faz o fetch do
   service worker, que escapa do CORS. A primeira é a honesta; a segunda troca configuração
   de servidor por permissão mais ampla no cliente.
2. **O rate limit por IP fica frágil.** Ele existe para conter abuso de escrita, e a
   extensão multiplica escritas por pessoa. Em rede com NAT todo mundo divide o IP e um
   usuário ativo consome a cota dos outros — o mesmo problema de chave compartilhada já
   visto no `X-Forwarded-For` da Etapa 5. Com a Etapa 11 pronta, limitar por conta quando
   houver sessão e por IP só no caminho anônimo.

**A credencial da extensão não é o cookie de sessão.** Extensão não compartilha cookie com o
site de forma confiável entre navegadores. O caminho é o token de API que ficou como
alternativa preterida na Etapa 11: a pessoa gera um, cola na extensão, e ele vale como
credencial daquele cliente — revogável sem derrubar a sessão do navegador.

## Etapa 14 — Painel do administrador

Uma página autenticada com os números do produto — criados nas últimas 24 h, total
acumulado, top códigos por acesso, taxa de erro — ao lado dos números de sistema que a
Etapa 7 já coleta.

Depois das Etapas 11 e 12: sem sessão não há como restringir o acesso, e sem os eventos de
clique metade dos números do produto não existe.

- [ ] Papel de administrador em `users` — sem isso, "autenticado" viraria "qualquer conta"
- [ ] `GET /v1/admin/stats`: criados por período, total, taxa de erro, top códigos
- [ ] Página no front, atrás de sessão com esse papel
- [ ] Números de sistema vindos do Prometheus, não recalculados no Postgres

🎯 Responder "quantos links nas últimas 24 h" e "onde está o gargalo" numa tela só.

**A separação que não pode ser perdida.** São duas fontes com naturezas diferentes: os
números de produto saem do Postgres/ClickHouse (verdade transacional, tem de ser exata) e os
de sistema saem do Prometheus (série temporal amostrada, aproximada por construção). Um
painel que mistura as duas como se fossem a mesma coisa mente nas duas — cada uma tem que
ser lida de onde vive, e ficar claro qual é qual.

**Cuidado com `COUNT(*)` nas últimas 24 h.** Numa tabela projetada para 365 bilhões de
linhas, essa consulta varre índice a cada carregamento do painel. Vira contador incremental
ou tabela de agregado diário — a mesma decisão de rollup que a Etapa 12 já enfrenta.

**Grafana já resolve os números de sistema.** Este painel existe para o que ele não tem:
dado de produto. Reimplementar gráfico de latência aqui seria trabalho duplicado com
resultado pior — o caminho é embutir o painel do Grafana ou apontar para ele.

## Etapa 15 — Confirmação de e-mail (Resend)

**Não é urgente: login já funciona sem confirmar e-mail.** Fica aqui, depois do núcleo, de
propósito. O que ela paga é fechar a enumeração do signup (a pendência da Etapa 11) — hoje
um trade-off aceito, não um bug. Enquanto não vier, o `409 EmailTaken` continua entregando
quais e-mails têm conta.

- [ ] Integração com **Resend** atrás de um tipo `Mailer`, com variante desabilitada — do
      mesmo jeito que `Cache::disabled()` e o `ClickSink` da Etapa 12. Local e teste **não
      enviam**: registram o link no log. Sem isso, todo teste de cadastro manda e-mail de
      verdade e gasta cota
- [ ] Coluna `email_verified_at timestamptz NULL` em `users`
- [ ] Token de confirmação: 128 bits aleatórios, no Redis com TTL, chave → `user_id`.
      Não é JWT — um token opaco de uso único é mais simples e revogável, e não há claim
      nenhuma que valha assinar
- [ ] `POST /v1/verify-email` consome o token, grava `email_verified_at`, apaga o token
- [ ] Reenvio: `POST /v1/resend-verification`, com rate limit próprio (cada envio custa uma
      chamada ao Resend e é vetor de e-mail bombing)

🎯 Cadastrar com um endereço novo e com um já cadastrado devolve **a mesma resposta HTTP** e
   o mesmo tempo; só o dono do endereço descobre qual foi, pela caixa de entrada.

**Como a enumeração some.** O signup passa a responder sempre `200 "verifique seu e-mail"`.
Endereço novo → cria a conta não-verificada e manda o link de confirmação. Endereço já
existente → **não cria nada** e manda um "alguém tentou cadastrar com seu e-mail". A
resposta ao cliente é idêntica nos dois casos; a diferença viaja só pelo canal que o
atacante não controla. O `409 EmailTaken` de hoje deixa de existir.

**O que "não verificado" bloqueia é decisão de produto, não técnica.** Duas posturas
coerentes: (a) não deixa entrar até confirmar — mais rígido, e trava quem não recebeu o
e-mail; (b) deixa usar e mostra um aviso, exigindo confirmação só para o reset de senha.
Para um encurtador, (b) atrita menos e não perde ninguém por causa de spam filter. A
escolha fica registrada quando for feita.

**E-mail é canal que você não controla.** Entrega não é garantida — spam, greylisting,
domínio novo sem reputação. Duas consequências: o fluxo **precisa** de reenviar, e o
domínio precisa de SPF/DKIM/DMARC configurados no Resend antes de qualquer envio valer.
Sem isso o link de confirmação cai em spam e a conta parece quebrada.

**Se o Resend cair, o cadastro não pode cair junto.** A conta é criada primeiro; o envio
vem depois e a falha dele não derruba o signup — a pessoa pede reenvio. Amarrar a criação
da conta ao sucesso do envio transforma um provedor de e-mail fora do ar numa
indisponibilidade do cadastro.

## Etapa 16 — Reset de senha (Resend)

Depois da Etapa 15 — reaproveita o `Mailer` já integrado. Duas etapas distintas de
propósito: confirmação prova que o endereço é seu; reset devolve o acesso quando a senha
se perde. São fluxos, tokens e telas diferentes, e juntar os dois num só esconde que as
regras de segurança de cada um também são diferentes.

- [ ] `POST /v1/forgot-password` recebe o e-mail e responde **sempre 200** — mesma defesa
      contra enumeração da Etapa 15. Se existe, manda o link; se não, não faz nada
- [ ] Token de reset: 128 bits, Redis, TTL **curto** (15–30 min) e **uso único** — some no
      primeiro uso. Mais curto que o de confirmação, porque dá acesso à conta, não só a
      prová-la
- [ ] `POST /v1/reset-password` valida o token, valida a nova senha (mesmas regras do
      signup), re-hasheia e grava
- [ ] **Ao trocar a senha, revoga todas as sessões** — é exatamente o `revoke_all` do índice
      reverso que a Etapa 11 já construiu. Quem redefine senha geralmente perdeu o controle
      da conta; deixar as sessões antigas vivas anularia o reset
- [ ] Rate limit próprio no `forgot-password`: cada chamada é um envio de e-mail (custo e
      e-mail bombing) e o Argon2 do reset é caro

🎯 Esqueci a senha → recebo o link → defino outra → entro com ela, e **toda sessão anterior
   deixou de valer**.

**O reset é o caminho de tomada de conta mais provável, então ele é o mais duro.** Token
curto, uso único, e revogação de tudo ao concluir. O e-mail de reset também deve avisar
"se não foi você, ignore" — sem link de ação, porque um link de "não fui eu" clicável vira
outro vetor.

**Não confirmar que o reset chegou também é anti-enumeração.** A tentação é responder "não
existe conta com esse e-mail" para ajudar quem digitou errado. Isso reabre exatamente o
oráculo que a Etapa 15 fechou. A resposta é sempre a mesma; quem não recebe, ou errou o
endereço, ou não tem conta — e o produto não diz qual.

---

## SonarQube no CI ✅

- [x] Job no `ci.yml` com `SonarSource/sonarqube-scan-action`
- [x] `fetch-depth: 0` no checkout — sem histórico completo o Sonar não calcula *new code*
- [x] `sonar-project.properties`: `sonar.sources=crates`, excluindo `target/`, `.sqlx/`,
      `dist/` e `fonts/`
- [x] Importar o que já temos em vez de duplicar análise: `cargo clippy
      --message-format=json` e LCOV do `cargo llvm-cov` (medido dentro do job de testes, que
      já tem Postgres e Redis de pé)
- [x] Scan pulado em PR de fork, onde o `SONAR_TOKEN` não existe — falharia sem dizer o
      motivo
- [ ] Adicionar `SONAR_TOKEN` em Settings → Secrets → Actions
- [ ] Confirmar o `projectKey` depois de importar o projeto (`organization` já está
      confirmada: é a mesma de `josemoura212/FC-sonar-node`)

**Por que aqui precisa de token, e no `Fc-sonar` não precisou.** Aquele repositório usa
*Automatic Analysis*, em que o SonarQube Cloud lê o repositório pelo app do GitHub, sem CI e
sem segredo. Esse modo cobre todas as linguagens **exceto Objective-C, Dart e Rust** — e não
importa cobertura nem relatório de linter externo. Ou seja: para este projeto ele não
funcionaria, e mesmo se funcionasse deixaria de fora justamente o clippy e o LCOV. O
`FC-sonar-node` já usa o caminho com scanner e token, que é o adotado aqui.

**Por que importar em vez de deixar o Sonar analisar por conta:** o portão de lint do CI já
é a afirmação mais rígida sobre este código — clippy com `pedantic` em deny. Se o Sonar
reportasse critério próprio, passariam a existir dois padrões em desacordo.

## Superfície de rede — o que generalizou

O trabalho em si é de infra deste deploy e vive em `docs/`, fora do git: depende do
provedor, do proxy e de quais serviços dividem o host. Fica aqui o que vale para
qualquer um.

**Regra órfã é regra aberta.** Metade das regras de entrada deste nó não tinha
processo nenhum do outro lado — sobras de um serviço desinstalado e de aplicações
que publicam só dentro da rede do Docker. Não são inócuas: são portas esperando
alguém subir algo naquele número. Cruzar o que o firewall abre com o que de fato
escuta é exercício de dez minutos, e devolve mais do que parece.

**Faixa aberta é decisão que ninguém tomou.** Com a faixa inteira de NodePort
liberada, todo Service novo nasce público sem ninguém escolher isso. E nenhum
precisava: um proxy no mesmo host alcança os serviços por dentro — tráfego que
entra pela bridge do runtime de container e nunca passa pelo firewall da interface
pública. Fechar não quebrou nada, verificado nos dois sentidos.

**Onde não há firewall de host, a regra do provedor é a camada inteira.** Vários
serviços de um nó Kubernetes escutam em `0.0.0.0` — kubelet, exporter de nó — e
ficam fora da internet só porque nada os alcança. Compensar com `iptables` local
não funciona: o k3s reescreve as regras e a alteração some no primeiro restart.
Cada regra aberta é a exposição inteira, sem segunda linha.

**Redigir endereço é higiene, não controle.** Tirar um IP do repositório impede
reintroduzi-lo e para de apontar o alvo, mas não recupera o que já esteve público
nem o que o DNS entrega de graça — um hostname do cluster vale o mesmo que o IP,
porque um `dig` converte um no outro. Sanitização parcial não sanitiza. O controle
é a regra de firewall; até ela existir, o endereço é conhecido.

O que **não** ficou exposto, e vale registrar: a porta das métricas não responde de
fora. O listener separado da Etapa 7 fez o trabalho dele.

**Cloudflare Tunnel — depois da Etapa 10, não antes.** Ele é melhor: zero portas
de entrada e IP de origem nunca exposto. Mas fecha o caminho que as Etapas 9 e 10
precisam, porque o k6 tem que medir a origem sem a CDN no meio — medir através
dela mediria a CDN. Com tudo fechado, o gerador de carga teria que rodar dentro
da VPS, disputando CPU com o alvo. É o erro que o estudo original cometeu e que
este projeto existe para não repetir.

## Pendência de segurança — o rate limit não segurava pelo caminho público ✅

Descoberto em 2026-07-29, ao verificar a reversão do build da Etapa 9. Mesma
imagem, mesmo momento, 60 requisições simultâneas ao `POST /v1/shorten`:

| Caminho | Resultado |
|---|---|
| Direto no serviço, sem CDN nem proxy | 40 × `200`, **20 × `429`** |
| Pelo caminho público, com CDN e proxy | **60 × `200`** |

O limite funciona; ele só não vê o cliente. A suspeita já estava anotada desde o
deploy — "com CDN e proxy o `X-Forwarded-For` chega como cadeia, vale conferir que
o primeiro é mesmo o do cliente" — e agora tem número.

- [x] Confirmar o que o `SmartIpKeyExtractor` está de fato lendo em produção
- [x] Fazer o proxy confiar nos ranges da CDN, para preservar o IP de origem
- [ ] Só então reavaliar `shorten_per_second` e `shorten_burst`, que hoje foram
      calibrados contra um limite que nunca chegou a atuar

**Resolvido em 2026-07-31.** O Traefik só preserva o `X-Forwarded-For` recebido quando o
peer está em `forwardedHeaders.trustedIPs`. Sem a lista ele descartava o header e escrevia
o endereço de quem conectou — atrás da Cloudflare, um edge diferente a quase cada
requisição. Uma flag no `command:` do proxy com as faixas da Cloudflare, e a mesma corrida
de 60 requisições concorrentes passou de **60 × 200** para **40 × 200 e 20 × 429** — igual
ao que só o NodePort direto produzia.

**Achado pela Etapa 12, não por este item.** O `unique` do dashboard de cliques contava ~1
visitante por clique: dois acessos com User-Agent idêntico, segundos de diferença, geravam
dois `visitor_hash`. O hash é `ip + user-agent + dia`, então só o IP podia ter variado — o
mesmo header, o mesmo defeito. Depois do ajuste os dois cliques colapsam em um hash com
`n=2`.

**A lição que generaliza, agora com a segunda metade:** não basta o extrator ler a ponta
certa da cadeia — o proxy precisa ter deixado a cadeia chegar. E um header estragado não
tem um sintoma só: aqui ele apareceu como limite que não limita **e** como métrica que
mente, em dois consumidores que nunca se falaram. Quando duas coisas não relacionadas
quebram junto, vale procurar a entrada que as duas leem.

**Ressalva:** o conserto é prospectivo. Os eventos gravados antes dele carregam um hash por
requisição, então o `unique` da janela de 30 dias continua inflado até eles saírem pelo TTL.

**E o conserto não é definitivo:** o arquivo é gerado pelo Coolify, e tanto o *Reset
Configuration* quanto um upgrade do Traefik o reescrevem, levando a flag junto sem avisar.
Os dois sintomas acima são o alarme de que ela caiu. Detalhes em `docs/infra/cluster.md`.

**Por que passa despercebido:** o teste ingênuo não distingue os dois casos. Trinta
requisições sequenciais a ~150 ms cada levam quatro segundos e meio, e a 5/s de
reposição o balde nunca esvazia — dá `200` nas trinta com ou sem limite. Só carga
**concorrente** revela. Foi exatamente o erro cometido aqui antes de refazer o teste
em paralelo.

**A lição que generaliza:** um rate limit por IP atrás de uma CDN só vale se a
cadeia de proxies preservar o IP de origem e o extrator ler a ponta certa dela. Do
contrário a chave passa a ser o edge da CDN — que varia por conexão — e cada
requisição vira um cliente novo. O limite existe, aparece na configuração, e não
limita nada.

## Pendência de configuração — quem carrega o `Settings` inteiro paga por ele

Descoberto em 2026-07-30, no primeiro deploy depois de ligar o analytics. O Job de
migração morreu em todas as tentativas:

```
invalid configuration: missing configuration field "analytics.clickhouse.password"
```

`APP_ANALYTICS__BACKEND` entrou no ConfigMap compartilhado, e o Job o herda por
`envFrom`. A senha, porém, foi ligada só como env de Secret no Deployment da API.
O migrador então exigiu um campo que ninguém lhe deu — sendo que ele carrega o
`Settings` completo e lê **apenas** `settings.database`.

O reparo imediato foi o Job sobrepor as duas chaves (`env` explícito vence
`envFrom`), com placeholder em vez do Secret real: dar uma credencial de ClickHouse
a um processo que não a usa só aumenta a área de exposição do Job.

- [ ] Fazer o migrador carregar só a seção de que precisa, para que nenhum binário
      futuro herde uma exigência que não tem
- [ ] Reavaliar se `analytics.clickhouse.password` deve continuar obrigatória — hoje
      a obrigatoriedade é o que faz um Secret ausente falhar alto em vez de degradar
      em silêncio, e essa parte é desejável

**Por que não deu prejuízo:** o passo de migração roda **antes** do rollout, então a
falha parou o deploy sem tocar nos Deployments em execução. Produção seguiu nas
imagens anteriores e nunca caiu. A ordem do workflow é o que transformou um erro de
configuração em deploy abortado em vez de indisponibilidade.

**A lição que generaliza:** um ConfigMap compartilhado é acoplamento. No momento em
que uma chave nova passa a ser obrigatória, ela vale para **todo** processo que monta
aquele ConfigMap — não só para o que a introduziu. Ligar uma feature em um Deployment
e esquecer o Job que divide a mesma configuração é uma falha que nenhum teste local
pega, porque local carrega um arquivo de configuração completo.

## Pendência de CI — quem segura um deploy não revisado

**O que foi tentado e revertido:** disparar o deploy em `pull_request` com `types: [closed]`,
para que push direto na `main` (possível com o bypass de admin) não publicasse nada. Falha
porque runs de `pull_request` **vindos de fork não recebem secrets** — um PR da comunidade,
depois de mergeado, quebraria por falta de `KUBECONFIG`. O repo é público; era questão de
tempo.

**O que ficou:** `push` na `main`, que roda no repositório base e sempre tem os secrets. Com o
ruleset, a única forma de a `main` andar é um PR mergeado, então o gatilho já descreve a
realidade.

**O que falta:** o gatilho nunca foi o lugar certo para barrar deploy não revisado — quem faz
isso é o **environment**. O job de deploy já declara `environment: production`; com *required
reviewers* configurado, ele fica pendente esperando aprovação humana, **inclusive num push de
bypass**. É mais forte do que o gatilho conseguia ser.

- [ ] Configurar *required reviewers* no environment `production` (Settings → Environments)
- [ ] Confirmar que um push de bypass fica pendente de aprovação em vez de publicar

**Descartado:** `pull_request_target` roda no contexto base *com* secrets — fazer checkout do
código do fork ali entrega as credenciais a quem abriu o PR. `workflow_run` resolveria, mas
acrescenta um workflow inteiro para um problema que o environment dissolve.

---

## Backlog pós-v1 (não começar antes da Etapa 10)

- [ ] Alta disponibilidade: 2º Nginx com keepalived/VRRP, Postgres standby com
      failover (Patroni/repmgr), Redis Sentinel (proteção contra thundering herd)
- [ ] Particionamento por tempo no Postgres (retenção de 10 anos = drop de partição)
- [ ] Sharding por hash do shortcode quando o volume justificar
- [ ] Write path: `synchronous_commit = off`, batch de inserts
- [x] ~~302 + analytics como modo opcional~~ — promovido a Etapa 12, com o 302 restrito
      às URLs que têm dono
