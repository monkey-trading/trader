/** One strategy: its state, a start or stop control, and its parameters. */
import type { Strategy } from "@/api/types";

import { ActionButton } from "./ActionButton";
import { useServices } from "./app-context";
import { ParamsForm } from "./ParamsForm";
import { useAction } from "./use-action";

/** Names for the strategy kinds the bot's config defines; an unknown kind shows as the bot spells it. */
const KIND_NAMES: Readonly<Record<string, string>> = {
  sma_crossover: "SMA crossover",
  ema_crossover: "EMA crossover",
  macd: "MACD signal-line crossover",
  rsi_reversion: "RSI reversion",
  bollinger_reversion: "Bollinger band reversion",
  donchian_breakout: "Donchian channel breakout",
};

/** A strategy of the account being shown. */
export function StrategyCard({ address, strategy }: { readonly address: string; readonly strategy: Strategy }) {
  const { api } = useServices();
  const toggle = useAction(address);
  const running = strategy.running;
  const name = KIND_NAMES[strategy.kind] ?? strategy.kind;
  return (
    <section aria-label={`${name} on ${strategy.market}`} className="rounded border border-line bg-surface p-3">
      <h3 className="font-semibold">{name}</h3>
      <p className="text-sm text-muted">
        {strategy.market}, {strategy.bar} bars
      </p>
      <p className="my-2">
        State: <strong>{running ? "Running" : "Stopped"}</strong>
        {strategy.warmedUp ? "" : ", warming up"}
        {strategy.target === null ? "" : `, wants ${strategy.target}`}
      </p>
      <ActionButton
        label={running ? "Stop strategy" : "Start strategy"}
        pendingLabel={running ? "Stopping…" : "Starting…"}
        action={toggle}
        call={() => api.setStrategyRunning(address, strategy.id, !running)}
      />
      {/* Keyed by the saved values, so a save (or a change from elsewhere) resets the drafts to them. */}
      <ParamsForm
        key={strategy.params.map((param) => param.value).join("|")}
        address={address}
        strategyId={strategy.id}
        params={strategy.params}
      />
    </section>
  );
}
