class Store:
    def get(self, key):
        return self.data[key]

    def put(self, key, value):
        self.data[key] = value


def helper():
    return 1
