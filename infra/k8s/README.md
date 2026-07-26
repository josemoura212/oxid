# Deploy no k3s (mangatrix)

Notas de deploy do oxid. Este diretório fica fora do git (`docs/` está no
`~/.gitignore_global`).

## Topologia

```
internet → Cloudflare (proxy ON, Origin Certificate)
             ↓
           Traefik (Docker, fora do cluster, gerenciado pelo Coolify)
             │  host.docker.internal:30091
             ↓
           Service NodePort  →  2 pods da API
                                   ├→ Service postgres  (StatefulSet + PVC local-path)
                                   └→ Service redis     (Deployment, sem PVC)
```

O Traefik **não** roda dentro do k3s — é o do Coolify, em Docker. Por isso o
Service é `NodePort` e não `Ingress`: ele alcança o nó por
`host.docker.internal`, o mesmo padrão de `k8s-learn` (30088),
`github-readme-stats` (30089) e `sensor-api` (30090).

## Estado (2026-07-26)

| | |
|---|---|
| k3s | v1.34.5, nó único `mangatrix` |
| IP público | 168.75.92.187 |
| API server | `https://k3s.mangatrix.net:6443` (acessível da internet) |
| StorageClass | `local-path` (default, `WaitForFirstConsumer`) |
| NodePort do oxid | 30091 |
| DNS | `oxid.uk` e `*.oxid.uk` → 168.75.92.187, **com proxy** |
| TLS | Cloudflare Origin Certificate, em `cert.yaml` do Coolify |

### O proxy da Cloudflare precisa ficar ligado

O Origin Certificate só é confiado pela própria Cloudflare. Com a nuvem cinza
(DNS only), o browser rejeita o certificado.

**Consequência para as Etapas 9 e 10:** a Cloudflare faz cache e rate limiting
próprios. Um k6 apontado para `https://oxid.uk` mede a Cloudflare, não o oxid —
os números de p95, hit rate e saturação seriam dela. O teste de carga tem que ir
direto no NodePort ou no IP da VPS, contornando o proxy.

## Deploy manual (primeira vez)

```bash
kubectl apply -f infra/k8s/00-namespace.yaml

# Secret do banco — nunca em arquivo versionado.
kubectl -n oxid create secret generic oxid-db \
  --from-literal=username=oxid \
  --from-literal=database=oxid \
  --from-literal=password="$(openssl rand -hex 24)"

kubectl apply -f infra/k8s/10-postgres.yaml
kubectl -n oxid rollout status statefulset/postgres

kubectl apply -f infra/k8s/20-redis.yaml
kubectl apply -f infra/k8s/30-api.yaml
kubectl -n oxid rollout status deployment/api
```

Depois cole `traefik-oxid.yaml` no Coolify (Servers → Proxy → Dynamic
Configurations) com o nome `oxid.yaml`, e recarregue o proxy.

A primeira imagem precisa existir antes do `apply` da API. Ou espere o workflow
`Deploy` rodar, ou publique à mão:

```bash
docker build -f infra/Dockerfile -t ghcr.io/josemoura212/oxid:latest .
echo "$GITHUB_TOKEN" | docker login ghcr.io -u josemoura212 --password-stdin
docker push ghcr.io/josemoura212/oxid:latest
```

O pacote precisa ser público no GHCR, ou o cluster vai precisar de um
`imagePullSecret`.

## Deploy automático

`.github/workflows/deploy.yml` roda a cada push na `main`:

1. Builda `infra/Dockerfile` e publica no GHCR, com tag `sha-<commit>` e `latest`
2. Roda as migrations como um `Job`, a partir da **mesma imagem**
3. `kubectl set image` **pelo digest**, não pela tag
4. Aguarda o rollout e faz um smoke test em `/health`

Deploy por digest e não por `latest` é o que torna o rollback possível: `latest`
não diz o que está rodando nem para onde voltar.

### Configuração única

```bash
kubectl apply -f infra/k8s/40-deploy-access.yaml
./infra/k8s/mint-kubeconfig.sh
```

O script imprime um kubeconfig em base64. Cole em **Settings → Secrets and
variables → Actions → New repository secret**, com o nome `KUBECONFIG`.

A credencial é de um ServiceAccount limitado ao namespace `oxid`, não a de
cluster-admin: ela pode atualizar o Deployment, rodar o Job de migração e ler
pods. Vazar isso não custa o cluster.

## Migrations

Rodam como `Job`, nunca no boot da aplicação — com 2 réplicas, as duas
tentariam ao mesmo tempo. O binário é o `oxid-migrate`, que sai da mesma imagem
da API, então schema e código que o espera são sempre o mesmo build.

Manualmente:

```bash
kubectl -n oxid delete job oxid-migrate --ignore-not-found
kubectl -n oxid create job oxid-migrate \
  --image=ghcr.io/josemoura212/oxid:latest -- /usr/local/bin/oxid-migrate
kubectl -n oxid logs job/oxid-migrate -f
```

## Verificação

```bash
kubectl -n oxid get pods -o wide
kubectl -n oxid logs -l app=api --tail=50

# NodePort direto, sem Traefik nem Cloudflare
curl -s http://10.0.0.43:30091/health

# Traefik, sem Cloudflare
curl -sk --resolve oxid.uk:443:168.75.92.187 https://oxid.uk/health

# Caminho completo
curl -s https://oxid.uk/health
```

Testar nessa ordem isola a camada com problema: se o NodePort responde e o
Traefik não, é proxy; se o Traefik responde e o domínio não, é Cloudflare.

## Pendências

- **Front não está no cluster.** Só a API. O complicador é que `GET /{code}` na
  raiz colide com os assets estáticos — ou o Traefik separa por path, ou a API
  serve os estáticos com `ServeDir` (uma origem só, resolve na raiz).
- **Sem observabilidade.** `/metrics` é a Etapa 7. Já existe um namespace
  `monitoring` no cluster.
- **Réplica única do Postgres**, sem standby e sem backup automatizado. O PVC é
  `local-path`, ou seja, disco do nó.
- **Sem NetworkPolicy.** Qualquer pod do cluster alcança o Postgres do oxid.
- **Rate limit e `X-Forwarded-For`.** Com Cloudflare + Traefik, o header chega
  com uma cadeia de IPs. Vale conferir que o primeiro é mesmo o do cliente, ou o
  limite passa a valer por proxy.
