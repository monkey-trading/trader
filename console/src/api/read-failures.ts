/** Why the console's reads of an account could not be had, by lower-case address, as immutable maps for components. */

/** How many times an account's full curve is read before the console stops trying until the next feed snapshot. */
export const CURVE_TRIES = 3;
/** How long after a failed read of the full curve it is tried again. */
export const CURVE_RETRY_MS = 5000;

/** The reasons fills and full equity curves could not be read, and how often a curve has been tried since it last worked. */
export class ReadFailures {
  private fillReasons: ReadonlyMap<string, string> = new Map();
  private curveReasons: ReadonlyMap<string, string> = new Map();
  private readonly curveTries = new Map<string, number>();

  /** Why each account's fills could not be read. */
  get fills(): ReadonlyMap<string, string> {
    return this.fillReasons;
  }

  /** Why each account's full curve could not be read. */
  get curves(): ReadonlyMap<string, string> {
    return this.curveReasons;
  }

  /** Notes that `key`'s fills could not be read. */
  failFills(key: string, reason: string): void {
    this.fillReasons = new Map(this.fillReasons).set(key, reason);
  }

  /** Forgets the reason `key`'s fills could not be read: they have been. */
  clearFills(key: string): void {
    this.fillReasons = new Map([...this.fillReasons].filter(([failed]) => failed !== key));
  }

  /** Notes that `key`'s full curve could not be read; how many tries it has had since it last worked. */
  failCurve(key: string, reason: string): number {
    this.curveReasons = new Map(this.curveReasons).set(key, reason);
    const tries = (this.curveTries.get(key) ?? 0) + 1;
    this.curveTries.set(key, tries);
    return tries;
  }

  /** Forgets the reason `key`'s full curve could not be read, and its tries. */
  clearCurve(key: string): void {
    this.curveReasons = new Map([...this.curveReasons].filter(([failed]) => failed !== key));
    this.curveTries.delete(key);
  }

  /** Forgets everything. */
  reset(): void {
    this.fillReasons = new Map();
    this.curveReasons = new Map();
    this.curveTries.clear();
  }
}
