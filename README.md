# fleet

Early prototype stage

## Advantages over existing configuration systems (NixOps/Morph)

- Modules can configure multiple hosts at once (I.e for wireguard/kubernetes installation)
- Secrets can be securely stored in Git (No one except target hosts can decrypt them)
