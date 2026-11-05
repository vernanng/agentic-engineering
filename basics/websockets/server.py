"""Echo server: pip install websockets, then python3 server.py"""
import asyncio

from websockets.asyncio.server import serve


async def echo(ws):
    async for message in ws:
        await ws.send(f"echo: {message}")


async def main():
    async with serve(echo, "localhost", 8765):
        print("listening on ws://localhost:8765")
        await asyncio.Future()  # run forever


if __name__ == "__main__":
    asyncio.run(main())
