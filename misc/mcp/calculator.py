from fastmcp import FastMCP

mcp = FastMCP("Demo 🚀")


@mcp.tool
def calculate(expression: str, _meta: dict | None = None) -> float:
    """
    Evaluate a pure arithmetic expression.
    Allowed: numbers, + - * / // % ** and parentheses
    """
    # _meta is accepted and ignored: the TypeScript Strands SDK puts
    # `_meta: {traceparent: ...}` inside the tool arguments for trace propagation, and
    # pydantic would otherwise reject it as an unexpected keyword and fail the call. The
    # Python SDK keeps it at the request level, where it belongs. Declared explicitly
    # rather than via **kwargs, which FastMCP refuses for tools.
    del _meta
    return eval(expression, {"__builtins__": None}, {})


def main():
    mcp.run()


if __name__ == "__main__":
    mcp.run()
