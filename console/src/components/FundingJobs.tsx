/** The funding jobs: each action's state until it lands. */
import type { FundingJob } from "@/api/funding-types";
import { shortAddress } from "@/lib/format";

import { CELL, DataTable } from "./DataTable";

const HEAD = ["Action", "Account", "State", "Result", "Transactions"] as const;
const STATES: Readonly<Record<FundingJob["state"], string>> = {
  waiting: "Waiting", running: "Running", landed: "Landed", failed: "Failed",
};

/** The jobs, newest first. */
export function FundingJobs({ jobs }: { readonly jobs: readonly FundingJob[] }) {
  if (jobs.length === 0) {
    return <p className="text-muted">No action has been started.</p>;
  }
  return (
    <DataTable label="Funding actions" head={HEAD}>
      {[...jobs].reverse().map((job) => (
        <tr key={job.id} className="border-t border-line">
          <td className={CELL}>{job.action}</td>
          <th scope="row" className={`${CELL} font-medium`}><span title={job.account}>{shortAddress(job.account)}</span></th>
          <td className={`${CELL} ${job.state === "failed" ? "text-loss" : ""}`}>{STATES[job.state]}</td>
          <td className="px-2 py-1">{job.detail}</td>
          <td className="px-2 py-1 font-mono text-xs break-all">{job.txs.join(" ")}</td>
        </tr>
      ))}
    </DataTable>
  );
}
