FROM python:3.12-slim
WORKDIR /app
COPY server.py .
COPY static ./static
ENV PORT=8000 HOST=0.0.0.0 LOOP_DB=/data/loop.db
VOLUME /data
CMD ["python", "server.py"]
