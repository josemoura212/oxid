#!/usr/bin/env bash
# Builds a kubeconfig for the `deployer` ServiceAccount and prints it base64
# encoded, ready to paste into the GitHub secret KUBECONFIG.
#
#   kubectl apply -f infra/k8s/40-deploy-access.yaml
#   ./infra/k8s/mint-kubeconfig.sh
#
# The output is a credential. It scrolls through your terminal and lands in the
# shell history of whatever you pipe it to — treat it accordingly.
set -euo pipefail

NAMESPACE=oxid
ACCOUNT=deployer
SECRET=deployer-token

# The public endpoint, not the one in your local kubeconfig — that may point at a
# LAN address the GitHub runner cannot reach.
SERVER="${K3S_SERVER:-https://k3s.mangatrix.net:6443}"

token=$(kubectl -n "$NAMESPACE" get secret "$SECRET" -o jsonpath='{.data.token}' | base64 -d)
ca=$(kubectl -n "$NAMESPACE" get secret "$SECRET" -o jsonpath='{.data.ca\.crt}')

cat <<YAML | base64 -w0
apiVersion: v1
kind: Config
clusters:
  - name: k3s
    cluster:
      server: ${SERVER}
      certificate-authority-data: ${ca}
contexts:
  - name: deployer
    context:
      cluster: k3s
      namespace: ${NAMESPACE}
      user: ${ACCOUNT}
current-context: deployer
users:
  - name: ${ACCOUNT}
    user:
      token: ${token}
YAML

echo >&2
echo "Paste the line above into the GitHub secret KUBECONFIG." >&2
