# Docker Basics

Docker packages an app plus everything it needs (runtime, libraries, config) into an **image**. Running an image gives a **container**: an isolated process that behaves the same on your laptop and on a server. Lighter than a VM — containers share the host kernel.

```text
Dockerfile --build--> image --run--> container
                          ^
                    registry (Docker Hub) --pull--> image
```

## Core commands

```bash
docker build -t myapp .          # build image from ./Dockerfile, tag it myapp
docker run -p 8000:8000 myapp    # run; map host port 8000 -> container 8000
docker run -d --name web myapp   # detached (background), named
docker ps                        # running containers
docker logs web                  # output of container "web"
docker exec -it web sh           # shell inside a running container
docker stop web && docker rm web # stop + delete container
docker images                    # local images
docker pull python:3.12-slim     # download an image without building
```

## Dockerfile

Instructions run top to bottom; each creates a cached layer, so put rarely-changing steps first.

```dockerfile
FROM python:3.12-slim        # base image
WORKDIR /app
COPY requirements.txt .
RUN pip install -r requirements.txt   # deps layer, cached
COPY . .
CMD ["python", "hello.py"]   # default command at container start
```

Rules of thumb:

- One app per container. Use **Docker Compose** (`docker compose up`) to run app + database together, defined in `compose.yaml`.
- Keep images small: slim/alpine bases, a `.dockerignore` (like `.gitignore`).
- Containers are disposable: store data in **volumes** (`-v data:/data`), not inside the container.
- Configure via environment variables: `docker run -e PORT=8000 ...`.

See [Dockerfile](Dockerfile) and [compose.yaml](compose.yaml). Try:

```bash
cd basics/docker && docker build -t basics-python . && docker run --rm basics-python
```

> **From the official docs:** Docker Docs define a container image as “a standardized package that includes all of the files, binaries, libraries, and configurations to run a container” — images are immutable and become containers at runtime. That image→container flow is the model for the commands and `Dockerfile` above.

## Sources

- [What is an image? — Docker Docs](https://docs.docker.com/get-started/docker-concepts/the-basics/what-is-an-image/) — image contents, layers, and image→container semantics.
- [Get started — Docker Docs](https://docs.docker.com/get-started/) — build, run, Compose, and registries.
