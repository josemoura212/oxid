# oxid

Encurtador de URL de alta performance escrito em Rust.

> 🇬🇧 [Read in English](README.md)

O encurtador não é o objetivo — **aprender Rust e system design sob carga real
é**. O projeto se espelha num estudo de caso que atingiu ~12.700 req/s
(100 milhões de URLs por dia) com p95 de leitura em milissegundos de um dígito.
Cada decisão está registrada com seu trade-off em
[`docs/DECISOES.md`](docs/DECISOES.md), inclusive as que se mostraram erradas.

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
- **Imutável.** Um código nunca muda de significado, e é por isso que o cache
  (etapa 5) não precisa de invalidação.

## Stack

| Camada | Escolha |
|---|---|
| HTTP | [Axum](https://github.com/tokio-rs/axum) sobre Tokio |
| Banco | PostgreSQL 18 via [sqlx](https://github.com/launchbadge/sqlx) (queries checadas em compile-time) |
| Cache | Redis, cache-aside com `allkeys-lru` *(etapa 5)* |
| Front | [Leptos](https://leptos.dev) CSR, compilado para WebAssembly |
| Config | Camadas YAML via [`config`](https://github.com/rust-cli/config-rs) + [`secrecy`](https://github.com/iqlusioninc/crates) |
| Observabilidade | `tracing` + Prometheus *(etapa 7)* |
| Balanceador | Nginx *(etapa 8)* |
| Teste de carga | k6 *(etapa 9)* |

## Estrutura

```
crates/
  oxid-api/       back: rotas, codec, repositório, config
  oxid-shared/    contrato da API — tipos compartilhados pelos dois lados
  oxid-web/       front (Leptos, wasm32)
configuration/    base.yaml → <ambiente>.yaml → variáveis APP_*
migrations/       migrations do sqlx
infra/            docker compose, imagem de desenvolvimento
docs/DECISOES.md  registro de decisões
```

O `oxid-shared` é onde Rust full-stack se paga: os tipos de request e response
vivem num lugar só, então **mudar um campo no back quebra o front em tempo de
compilação**. O contrato é garantido pelo compilador, não por teste de
integração.

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

Os dois serviços fazem hot reload a partir do host: o fonte vem por bind mount, o
`watchexec` reinicia a API e o `trunk` recompila o front. `Ctrl+C` derruba tudo em
bem menos de um segundo.

### Sem Docker

O Postgres continua vindo do compose; o resto roda nativo.

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

## Desenvolvimento

```bash
# testes (Postgres precisa estar no ar — cada teste ganha um banco temporário)
cargo test

# lint: fmt, back, e o front no target wasm
cargo fmt --all --check \
  && cargo clippy --workspace --exclude oxid-web --all-targets -- -D warnings \
  && cargo clippy -p oxid-web --target wasm32-unknown-unknown --all-targets -- -D warnings
```

Warning aqui é erro. O clippy roda com `pedantic` em deny, e `unwrap`, `expect`,
`panic`, indexação e aritmética não-checada são todos deny fora dos testes — todo
panic precisa ser decisão deliberada e justificada.

O front exige rodada própria do clippy: lints que só existem em `wasm32` ficam
invisíveis a partir do target do host.

### Mudando queries

O `sqlx` valida o SQL contra o schema real em tempo de compilação. Depois de mexer
numa query ou migration:

```bash
cargo sqlx prepare
```

Isso atualiza o `.sqlx/`, que é versionado para o CI compilar sem banco
(`SQLX_OFFLINE=true`).

## Roadmap

| Etapa | Status |
|---|---|
| 1. Fundação async | ✅ |
| 2. Base62 + bijeção ofuscadora | ✅ |
| 3. Persistência com sqlx | ✅ |
| 4. Rotas de escrita e leitura | ✅ |
| 5. Cache Redis (cache-aside + cache negativo) | |
| 6. Configuração e dimensionamento | ✅ (antecipada) |
| 7. Observabilidade (Prometheus + Grafana) | |
| 8. Topologia completa (2 instâncias atrás do Nginx) | |
| 9. Teste de carga com k6 | |
| 10. Ciclo de otimização até escala total | |

Detalhes em [`ROADMAP.md`](ROADMAP.md).

## Licença

MIT
