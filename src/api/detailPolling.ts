// Count distinct responses, not renders. A persistently unknown merge state
// stays live without asking for the full detail every three seconds forever.
export class DetailPollBackoff {
  private head: string | null = null;
  private receipt = -1;
  private attempts = -1;

  delay(head: string, unknown: boolean, receipt: number): number | false {
    if (!unknown || head !== this.head) {
      this.head = head;
      this.receipt = -1;
      this.attempts = -1;
    }
    if (!unknown) return false;
    if (receipt !== this.receipt) {
      this.receipt = receipt;
      this.attempts = Math.min(this.attempts + 1, 5);
    }
    return Math.min(3_000 * 2 ** this.attempts, 60_000);
  }
}
