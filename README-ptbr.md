# oxid

Encurtador de URL de alta performance escrito em Rust. No ar em
**[oxid.uk](https://oxid.uk)**.

> 🇬🇧 [Read in English](README.md)

O encurtador não é o objetivo — **aprender Rust e system design sob carga real
é**. O projeto se espelha num estudo de caso que atingiu ~12.700 req/s
(100 milhões de URLs por dia) com p95 de leitura em milissegundos de um dígito.

## Metas de projeto

| Requisito | Número |
|---|---|
| URLs criadas por dia | 100 milhões (~1.157 escritas/s) |
| Leituras | ~11.574/s (proporção 1:10) |
| Tamanho do shortcode | 7 caracteres Base62 (62⁷ ≈ 3,5 trilhões) |
| Retenção | 10 anos (~365 bilhões de registros) |
| Latência de leitura | p95 < 50 ms |
| Erros sob pico de 2–3× | zero |

## Como um shortcode é gerado

```
id bigserial  →  ofuscação  →  encode Base62  →  pad para 7  →  eDrBKMi
```

O id do banco passa por uma **bijeção** — multiplicação modular por um primo
coprimo de 62⁷ — então ids consecutivos caem longe um do outro e a sequência não
é deduzível a partir do código. Por ser bijeção, colisão é impossível por
construção: não há loop de retry nem verificação de unicidade sobre o código.

A resolução é o mesmo caminho ao contrário, terminando numa busca por chave
primária.

Duas propriedades valem saber:

- **Idempotente.** A mesma URL longa sempre devolve o mesmo código, garantido por
  unique constraint no Postgres — não pela aplicação. Requisições concorrentes
  para a mesma URL não conseguem gerar dois códigos.
- **Imutável.** Um código nunca muda de significado, e é por isso que o cache não
  precisa de invalidação nem carrega TTL.

## Stack

| Camada | Escolha |
|---|---|
| HTTP | [Axum](https://github.com/tokio-rs/axum) sobre Tokio |
| Banco | PostgreSQL 18 via [sqlx](https://github.com/launchbadge/sqlx) (queries checadas em compile-time) |
| Cache | Redis, cache-aside com `allkeys-lru`, mais cache negativo |
| Rate limit | [`tower_governor`](https://github.com/benwis/tower-governor), só na escrita |
| Front | [Leptos](https://leptos.dev) CSR compilado para WebAssembly, servido por nginx |
| Config | Camadas YAML via [`config`](https://github.com/rust-cli/config-rs) + [`secrecy`](https://github.com/iqlusioninc/crates) |
| Produção | k3s em arm64, GitHub Actions, Traefik na frente |
| Observabilidade | `tracing` + Prometheus *(etapa 7)* |
| Teste de carga | k6 *(etapa 9)* |

Leitura é ilimitada de propósito: o redirect é o caminho que o cache absorve e o
que as etapas 9-10 empurram a 11k req/s. Limitá-lo puniria exatamente o tráfego
que o sistema existe para servir.

## Estrutura

```
crates/
  oxid-api/       back: rotas, codec, repositório, cache, config
  oxid-shared/    contrato da API — tipos compartilhados pelos dois lados
  oxid-web/       front (Leptos, wasm32) + fontes subsetadas
configuration/    base.yaml → <ambiente>.yaml → variáveis APP_*
migrations/       migrations do sqlx
infra/            Dockerfiles, compose, nginx, manifests do k8s
.github/          CI (todo push e PR) e deploy (só na main)
```

O `oxid-shared` é onde Rust full-stack se paga: os tipos de request e response
vivem num lugar só, então **mudar um campo no back quebra o front em tempo de
compilação**. O contrato é garantido pelo compilador, não por teste de
integração.

> O registro de decisões vive em `docs/DECISOES.md` e **não está neste
> repositório** — `docs/` é ignorado globalmente na máquina do autor. Todas as
> decisões estão resumidas aqui ou no `ROADMAP.md`.

## Front

Uma página: cola uma URL, recebe um código. Enquanto você digita, um medidor
mostra o tamanho atual colapsando sobre os sete caracteres em que ele vai virar.

Os links criados ficam no **`localStorage`**, então fechar a aba não perde nada.
Não é cookie: um cookie do domínio viajaria em toda requisição, inclusive em cada
redirect `/{code}` — bytes a mais justamente no caminho em torno do qual o
sistema inteiro é ajustado. A lista nunca toca a rede, o que também significa que
ela não acompanha outro navegador ou aparelho, e limpar os dados do site a apaga.

Remover um link da lista **não** o desativa. Códigos são imutáveis e
compartilhados: a mesma URL longa gera o mesmo código para todo mundo, então
"apagar" seria apagar o link de outra pessoa. É também o que permite o redirect
ser 301.

A tipografia é JetBrains Mono, self-hosted e subsetada para ASCII mais dez
símbolos — 5 KB por peso, contra ~90 KB da família completa.

## Rodando

Precisa de Docker. O resto vive dentro dos containers.

```bash
cp .env.example .env

docker compose --env-file .env \
  -f infra/docker-compose.yml \
  -f infra/docker-compose.local.yml up
```

| Serviço | URL |
|---|---|
| API | http://127.0.0.1:3000 |
| Front | http://127.0.0.1:8080 |
| Postgres | `localhost:5432` |
| Redis | `localhost:6379` |

Os dois serviços fazem hot reload a partir do host: o fonte vem por bind mount, o
`watchexec` reinicia a API e o `trunk` recompila o front. `Ctrl+C` derruba tudo em
bem menos de um segundo.

### Sem Docker

Postgres e Redis continuam vindo do compose; o resto roda nativo.

```bash
docker compose --env-file .env -f infra/docker-compose.yml up -d
cargo sqlx migrate run

cargo run -p oxid-api                      # API
cd crates/oxid-web && trunk serve          # front
```

## API

### `POST /v1/shorten`

```bash
curl -X POST http://127.0.0.1:3000/v1/shorten \
  -H 'Content-Type: application/json' \
  -d '{"url":"https://exemplo.com/um/caminho/bem/longo"}'
```

```json
{
  "code": "eDrBKMi",
  "short_url": "http://127.0.0.1:3000/eDrBKMi",
  "long_url": "https://exemplo.com/um/caminho/bem/longo"
}
```

Só `http` e `https` são aceitos. URLs `javascript:` e `data:` são sintaticamente
válidas, mas transformariam o encurtador em vetor de XSS servido de um domínio
confiável — por isso o scheme é checado explicitamente.

### `GET /{code}`

Responde **301** com a URL original no `Location`. O redirect fica na raiz de
propósito: `/v1/urls/` gastaria nove caracteres numa URL cujo trabalho inteiro é
ser curta.

Código malformado responde 404, não 400 — separar os dois vazaria o formato do
shortcode para quem sondasse o endpoint.

### `GET /health`

### Erros

Os erros seguem a [RFC 9457](https://www.rfc-editor.org/rfc/rfc9457), servidos
como `application/problem+json`:

```json
{
  "type": "/problems/invalid-url",
  "title": "Invalid URL",
  "status": 400,
  "detail": "only http and https urls are accepted"
}
```

`title` é estável e serve para o cliente fazer match; `detail` descreve aquela
ocorrência específica. Erros de banco nunca chegam ao cliente — nomes de tabela,
coluna e constraint entregariam um mapa do schema.

## Observabilidade

As métricas são servidas em **porta própria**, nunca no router público:

```bash
curl http://127.0.0.1:9090/metrics
```

A separação é deliberada. O Traefik encaminha para este serviço tudo que não é o
front, então uma rota `/metrics` publicaria na internet volume de requisições,
distribuição de latência e comportamento do cache. No cluster a porta é declarada
no Deployment e fica de fora do Service.

Prometheus e Grafana vivem num compose separado, para o `up` do dia a dia
continuar leve:

```bash
docker compose --env-file .env \
  -f infra/docker-compose.yml \
  -f infra/docker-compose.local.yml \
  -f infra/docker-compose.observability.yml up
```

| Serviço | URL |
|---|---|
| Grafana (anônimo, admin) | http://127.0.0.1:3001 |
| Prometheus | http://127.0.0.1:9091 |

O dashboard é provisionado por arquivo em `infra/grafana/provisioning/` — p50/p95/p99
por rota, req/s por status, hit rate do cache, lookups por desfecho e conexões do
pool. Um dashboard montado na interface viveria só no volume do Grafana e não
passaria por revisão em PR.

Três detalhes que valem saber:

- **O label de rota vem do path casado**, não da URL. Em `/{code}` os dois diferem
  a cada request, e rotular com o path real criaria uma série temporal por
  shortcode.
- **Os buckets de latência são escolhidos, não os padrão.** A meta da Etapa 10 é
  p95 < 50 ms, então a resolução fica abaixo de 100 ms, que é onde as respostas
  caem.
- **Os gauges do pool são lidos no momento do scrape**, então nunca estão mais
  velhos que ele. `total - idle` é o número que importa: grudado em
  `max_connections` significa fila no pool, não no banco.

## Desempenho medido

Etapa 9, contra o deploy no ar, num nó **arm64 de 2 vCPU que ele não tem só para
si** — a mesma máquina roda o banco, o cache, o proxy, a pilha de monitoramento e
projetos alheios. Os scripts estão em [`infra/k6`](infra/k6).

| | |
|---|---|
| Sustentado, com todos os thresholds verdes | **2.546 req/s** (2.315 leituras + 231 escritas) |
| p95 de leitura, no cliente | **23,2 ms** — dos quais 19 ms são ida e volta de rede |
| p95 de leitura, no servidor | **~4 ms** |
| Erros | **zero**, em toda escala testada |
| Hit rate do cache sob carga | **100%** — 5.613 hits/s, nenhum miss |
| Escritas, medidas ao criar o pool | **1.230/s**, zero falhas |

O zero erros vale mesmo onde a latência não valeu: a 6.366 req/s o p95 degradou
para 186 ms, mas 1,2 milhão de requisições não produziram nenhum 5xx nem uma
conexão recusada. O serviço enfileira em vez de quebrar.

**O gargalo é CPU**, e a medição descarta os suspeitos de sempre. O pool de
conexões ficou ocioso o tempo todo — na proporção 10:1 as leituras nunca chegam ao
banco. Toda consulta ao cache foi acerto. O PSI do nó mostrou todas as tarefas do
cluster bloqueadas esperando CPU em 17% do tempo.

O joelho é estreito: o p95 salta de 23 ms para 109 ms com 1,5x de carga. Isso é
fila cruzando a saturação, não degradação suave — e é por isso que a Etapa 10 tem
de acrescentar capacidade em vez de ajustar configuração. A escala total exigiria
cerca de 2.780 mCPU contra os 2.000 que o nó tem inteiro.

Uma nota de método que generaliza: a carga foi gerada de fora do datacentro, a 19
ms de distância. Isso acabou não importando, porque a diferença entre os percentis
do cliente e do servidor encolhe exatamente onde a fila começa — 186,5 ms contra
181,7 ms na saturação. Medir dos dois lados é o que torna o resultado defensável,
já que a telemetria do servidor não depende do gerador.

## Desenvolvimento

```bash
# testes (Postgres precisa estar no ar — cada teste ganha um banco temporário)
cargo test --workspace --exclude oxid-web

# lint: fmt, back, e o front no target wasm
cargo fmt --all --check \
  && cargo clippy --workspace --exclude oxid-web --all-targets -- -D warnings \
  && cargo clippy -p oxid-web --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Warning aqui é erro. O clippy roda com `pedantic` em deny, e `unwrap`, `expect`,
`panic`, indexação e aritmética não-checada são todos deny fora dos testes — todo
panic precisa ser decisão deliberada e justificada.

O front exige rodada própria do clippy: lints que só existem em `wasm32` ficam
invisíveis a partir do target do host. Os dois crates são lib + binário em vez de
binário puro, porque num binário puro `unreachable_pub` e `redundant_pub_crate` se
contradizem.

### Mudando queries

O `sqlx` valida o SQL contra o schema real em tempo de compilação. Depois de mexer
numa query ou migration:

```bash
cargo sqlx prepare
```

Isso atualiza o `.sqlx/`, que é versionado para o CI compilar sem banco
(`SQLX_OFFLINE=true`).

## Deploy

Todo push na `main` builda as duas imagens num **runner arm64 nativo**, roda as
migrations como `Job` do Kubernetes e faz o rollout da API e depois do front
**por digest** — nunca por `latest`, que não diz o que está rodando nem para onde
voltar. O CI roda em todo pull request.

Em `infra/k8s/` estão os manifests que dá para adaptar: namespace, Postgres,
Redis, API, front, o `Job` de migração e o RBAC que o deploy usa. O que
**não** está ali, de propósito, é a cola com um ambiente específico — rotas de
proxy, hostnames, medições do nó. Cada deploy difere justamente aí, e a fiação
de outra pessoa é ruído, não ponto de partida.

## Roadmap

| Etapa | Status |
|---|---|
| 1. Fundação async | ✅ |
| 2. Base62 + bijeção ofuscadora | ✅ |
| 3. Persistência com sqlx | ✅ |
| 4. Rotas de escrita e leitura | ✅ |
| 5. Cache Redis (cache-aside + cache negativo) | ✅ |
| 5.1 Links salvos no navegador | ✅ |
| 5.2 Acertos do Lighthouse | parcial — contraste, ARIA e alvos de toque prontos |
| 6. Configuração e dimensionamento | ✅ (antecipada) |
| 7. Observabilidade (Prometheus + Grafana) | ✅ |
| 8. O teto do nó único | ✅ |
| 9. Teste de carga com k6 | ✅ — teto medido, gargalo identificado |
| 10. Ciclo de otimização até escala total | |
| 11. Contas e sessão | |
| 12. Analytics de clique | |

Detalhes em [`ROADMAP.md`](ROADMAP.md).

## Licença

MIT
