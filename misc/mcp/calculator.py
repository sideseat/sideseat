from fastmcp import FastMCP

mcp = FastMCP("Demo 🚀")


@mcp.tool
def calculate(expression: str) -> float:
    """
    Evaluate a pure arithmetic expression.
    Allowed: numbers, + - * / // % ** and parentheses
    """
    return eval(expression, {"__builtins__": None}, {})


def main():
    mcp.run()


if __name__ == "__main__":
    mcp.run()
