Configure Tekton CI
===================

- Install `git-clone` task

```bash
kubectl apply -f https://raw.githubusercontent.com/tektoncd/catalog/main/task/git-clone/0.5/git-clone.yaml
```

- Install trigger, pipelines and tasks

```bash
kubectl apply -f ./tekton
```
