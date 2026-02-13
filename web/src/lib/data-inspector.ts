export type ValueType = "string" | "number" | "boolean" | "null" | "object" | "array";

export interface DataInspectorRow {
  id: string;
  path: string;
  name: string;
  value: unknown;
  type: ValueType;
  depth: number;
  hasChildren: boolean;
  childCount?: number;
}

export function getValueType(value: unknown): ValueType {
  if (value === null || value === undefined) return "null";
  if (Array.isArray(value)) return "array";
  if (typeof value === "object") return "object";
  if (typeof value === "string") return "string";
  if (typeof value === "number") return "number";
  if (typeof value === "boolean") return "boolean";
  return "string";
}

export function getChildCount(value: unknown): number {
  if (value === null || value === undefined) return 0;
  if (Array.isArray(value)) return value.length;
  if (typeof value === "object") return Object.keys(value).length;
  return 0;
}

export function formatValue(value: unknown, type: ValueType): string {
  if (type === "null") return "null";
  if (type === "string") return String(value);
  if (type === "number" || type === "boolean") return String(value);
  if (type === "object" || type === "array") {
    const count = getChildCount(value);
    if (count === 0) return type === "array" ? "empty array" : "empty object";
    return `${count} ${count === 1 ? "item" : "items"}`;
  }
  return String(value);
}

export function transformToRows(
  data: Record<string, unknown> | unknown[],
  parentPath = "",
  depth = 0,
  maxDepth = 10,
  seen = new WeakSet<object>(),
): DataInspectorRow[] {
  if (depth > maxDepth) return [];

  if (typeof data === "object" && data !== null) {
    if (seen.has(data)) return [];
    seen.add(data);
  }

  const rows: DataInspectorRow[] = [];
  const entries = Array.isArray(data)
    ? data.map((v, i) => [String(i), v] as const)
    : Object.entries(data);

  for (const [key, value] of entries) {
    const path = parentPath ? `${parentPath}.${key}` : key;
    const type = getValueType(value);
    const hasChildren = (type === "object" || type === "array") && getChildCount(value) > 0;

    rows.push({
      id: path,
      path,
      name: key,
      value,
      type,
      depth,
      hasChildren,
      childCount: hasChildren ? getChildCount(value) : undefined,
    });
  }

  return rows;
}

export function getChildRows(
  row: DataInspectorRow,
  maxDepth = 10,
  seen = new WeakSet<object>(),
): DataInspectorRow[] {
  if (!row.hasChildren || row.value === null || row.value === undefined) return [];
  return transformToRows(
    row.value as Record<string, unknown> | unknown[],
    row.path,
    row.depth + 1,
    maxDepth,
    seen,
  );
}

/**
 * Recursively flatten a value.
 * - Single-item arrays → unwrap to the item
 * - Single-key objects with primitive value → unwrap to the primitive
 * - Recursively processes nested structures
 */
function flattenValue(value: unknown): unknown {
  // Primitive - return as is
  if (value === null || typeof value !== "object") {
    return value;
  }

  // Array - flatten single-item arrays
  if (Array.isArray(value)) {
    if (value.length === 1) {
      return flattenValue(value[0]);
    }
    return value;
  }

  // Object - check for single-key with primitive, otherwise recurse
  const obj = value as Record<string, unknown>;
  const keys = Object.keys(obj);

  if (keys.length === 1) {
    const childValue = obj[keys[0]];
    // Single-key object with primitive → unwrap
    if (childValue === null || typeof childValue !== "object") {
      return childValue;
    }
    // Single-key object with single-item array → unwrap both levels
    if (Array.isArray(childValue) && childValue.length === 1) {
      return flattenValue(childValue[0]);
    }
  }

  // Recurse into object properties
  const result: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    result[k] = flattenValue(v);
  }
  return result;
}

/**
 * Flatten data structure recursively.
 * - Single-item arrays → unwrapped to the item
 * - Single-key objects with primitive value → unwrapped to the primitive
 * - Recursively processes all nested structures
 */
export function flattenSingleChildren(data: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {};

  for (const [key, value] of Object.entries(data)) {
    result[key] = flattenValue(value);
  }

  return result;
}

export function getAllExpandablePaths(
  data: Record<string, unknown> | unknown[],
  parentPath = "",
  depth = 0,
  maxDepth = 10,
  seen = new WeakSet<object>(),
): string[] {
  if (depth > maxDepth) return [];

  if (typeof data === "object" && data !== null) {
    if (seen.has(data)) return [];
    seen.add(data);
  }

  const paths: string[] = [];
  const entries = Array.isArray(data)
    ? data.map((v, i) => [String(i), v] as const)
    : Object.entries(data);

  for (const [key, value] of entries) {
    const path = parentPath ? `${parentPath}.${key}` : key;
    const type = getValueType(value);
    const hasChildren = (type === "object" || type === "array") && getChildCount(value) > 0;

    if (hasChildren) {
      paths.push(path);
      paths.push(
        ...getAllExpandablePaths(
          value as Record<string, unknown> | unknown[],
          path,
          depth + 1,
          maxDepth,
          seen,
        ),
      );
    }
  }

  return paths;
}
