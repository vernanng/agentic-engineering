"""Client for server.py: pip install websockets, then python3 client.py"""
import asyncio

from websockets.asyncio.client import connect


async def main():
    async with connect("ws://localhost:8765") as ws:
        await ws.send("hello")
        print(await ws.recv())


if __name__ == "__main__":
    asyncio.run(main())
