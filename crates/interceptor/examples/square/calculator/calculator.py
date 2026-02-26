import wit_world

class WitWorld(wit_world.WitWorld):
    def add(self, a: int, b: int) -> int:
        return a + b

    def subtract(self, a: int, b: int) -> int:
        return a - b
