/**
 * Type utilities and helpers for the CXP SDK
 */

/**
 * Make all properties of T required and non-nullable
 */
export type Required<T> = {
  [P in keyof T]-?: NonNullable<T[P]>;
};

/**
 * Extract keys of T where the property type is U
 */
export type KeysOfType<T, U> = {
  [K in keyof T]: T[K] extends U ? K : never;
}[keyof T];

/**
 * Make specified keys K of T optional
 */
export type PartialBy<T, K extends keyof T> = Omit<T, K> & Partial<Pick<T, K>>;

/**
 * Make specified keys K of T required
 */
export type RequiredBy<T, K extends keyof T> = Omit<T, K> & Required<Pick<T, K>>;

/**
 * Recursively make all properties of T readonly
 */
export type DeepReadonly<T> = {
  readonly [P in keyof T]: T[P] extends object ? DeepReadonly<T[P]> : T[P];
};

/**
 * Recursively make all properties of T optional
 */
export type DeepPartial<T> = {
  [P in keyof T]?: T[P] extends object ? DeepPartial<T[P]> : T[P];
};

/**
 * Extract the awaited return type of an async function
 */
export type AsyncReturnType<T extends (...args: any) => Promise<any>> = T extends (
  ...args: any
) => Promise<infer R>
  ? R
  : never;

/**
 * Convert snake_case string literal type to camelCase
 */
type CamelCase<S extends string> = S extends `${infer P}_${infer Q}`
  ? `${Lowercase<P>}${Capitalize<CamelCase<Q>>}`
  : S;

/**
 * Convert snake_case object keys to camelCase
 */
export type CamelCaseKeys<T> = {
  [K in keyof T as CamelCase<K & string>]: T[K];
};

/**
 * Convert camelCase string literal type to snake_case
 */
type SnakeCase<S extends string> = S extends `${infer T}${infer U}`
  ? `${T extends Capitalize<T> ? "_" : ""}${Lowercase<T>}${SnakeCase<U>}`
  : S;

/**
 * Convert camelCase object keys to snake_case
 */
export type SnakeCaseKeys<T> = {
  [K in keyof T as SnakeCase<K & string>]: T[K];
};

/**
 * Utility to ensure exhaustive type checking in switch statements
 */
export function assertNever(value: never): never {
  throw new Error(`Unexpected value: ${value}`);
}

/**
 * Type guard to check if a value is defined (not null or undefined)
 */
export function isDefined<T>(value: T | null | undefined): value is T {
  return value !== null && value !== undefined;
}

/**
 * Type guard to check if a value is a string
 */
export function isString(value: unknown): value is string {
  return typeof value === "string";
}

/**
 * Type guard to check if a value is a number
 */
export function isNumber(value: unknown): value is number {
  return typeof value === "number" && !Number.isNaN(value);
}

/**
 * Type guard to check if a value is an object
 */
export function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Type guard to check if a value is an array
 */
export function isArray<T>(value: unknown): value is T[] {
  return Array.isArray(value);
}

/**
 * Utility type for JSON-serializable values
 */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/**
 * Utility type for function that can be used as a predicate
 */
export type Predicate<T> = (value: T) => boolean;

/**
 * Utility type for a comparator function
 */
export type Comparator<T> = (a: T, b: T) => number;

/**
 * Utility type for a mapper function
 */
export type Mapper<T, U> = (value: T) => U;

/**
 * Result type for operations that can fail
 */
export type Result<T, E = Error> =
  | { success: true; value: T }
  | { success: false; error: E };

/**
 * Create a success result
 */
export function success<T>(value: T): Result<T, never> {
  return { success: true, value };
}

/**
 * Create an error result
 */
export function failure<E>(error: E): Result<never, E> {
  return { success: false, error };
}

/**
 * Unwrap a Result, throwing if it's an error
 */
export function unwrap<T, E>(result: Result<T, E>): T {
  if (result.success) {
    return result.value;
  }
  throw result.error;
}

/**
 * Get the value from a Result or a default value
 */
export function unwrapOr<T, E>(result: Result<T, E>, defaultValue: T): T {
  return result.success ? result.value : defaultValue;
}

/**
 * Map a Result value using a function
 */
export function mapResult<T, U, E>(
  result: Result<T, E>,
  fn: Mapper<T, U>
): Result<U, E> {
  return result.success ? success(fn(result.value)) : result;
}
