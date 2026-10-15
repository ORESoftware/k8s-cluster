# Deploying tor-server on Kubernetes

Deterministic bootstrap: relay identity keys are generated ahead of time so the
client directory (which lists each relay's public key) is known before anything
starts.

## 1. Build & push the image

```sh
docker build -t oresoftware/tor-server:0.1.0 .
docker push oresoftware/tor-server:0.1.0
```

## 2. Generate three relay keys

```sh
for r in a b c; do TOR_KEY_FILE=./relay-$r.key tor-server keygen; done
# note each printed `pubkey:` value
```

## 3. Create namespace, secrets, and directory

```sh
kubectl apply -f k8s/namespace.yaml

# relay secret keys (keeps raw keys out of git)
for r in a b c; do
  kubectl -n anon-proxy create secret generic tor-relay-$r-key \
    --from-file=relay.key=./relay-$r.key
done

# A shared, high-entropy overlay membership secret is mandatory for the
# non-loopback relay manifests.
openssl rand -base64 32 > network.secret
kubectl -n anon-proxy create secret generic tor-network-secret \
  --from-file=network.secret=./network.secret

# Defense-in-depth credentials for the remote SOCKS listener and dashboard.
# RFC 1929 does not encrypt SOCKS credentials, so keep the Service private.
openssl rand -base64 32 > socks.password
openssl rand -base64 32 > ui.token
kubectl -n anon-proxy create secret generic tor-client-auth \
  --from-file=socks.password=./socks.password \
  --from-file=ui.token=./ui.token

# directory ConfigMap: copy the example, paste the three pubkeys, then apply
cp k8s/directory-configmap.example.yaml k8s/directory-configmap.yaml
$EDITOR k8s/directory-configmap.yaml
kubectl apply -f k8s/directory-configmap.yaml
```

## 4. Deploy relays and client

```sh
kubectl apply -f k8s/relays.yaml
kubectl apply -f k8s/client.yaml
kubectl apply -f k8s/networkpolicy.yaml
```

## 5. Route traffic

The default NetworkPolicy permits only pods in `anon-proxy`. Add a narrowly
scoped ingress source for each authorized workload namespace, then configure
its SOCKS5 client with username `tor` and the `socks.password` value:

```
socks5h://tor-client.anon-proxy.svc.cluster.local:9050
```

For example, without putting the password in a URL or process listing:

```sh
curl --proxy socks5h://tor-client.anon-proxy.svc.cluster.local:9050 \
  --proxy-user "tor:${TOR_SOCKS_PASSWORD}" https://example.com/
```

Files ending in `.example.yaml` are templates — copy, fill in, and apply the
non-example version (which is git-ignored via the repo `.gitignore` if you name
it `directory-configmap.yaml`). Never commit real key material.
