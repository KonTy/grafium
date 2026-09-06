<script lang="ts">
  interface Member {
    block_id: string;
    order_index: number;
    page_title: string;
  }

  interface Props {
    kind: string;
    members: Member[];
    memberCount?: number;
    onNavigate: (title: string) => void;
  }

  let { kind, members, memberCount, onNavigate }: Props = $props();

  const kindLabel = $derived(
    kind
      .trim()
      .replace(/[_-]+/g, " ")
      .replace(/\b\w/g, (letter) => letter.toUpperCase()),
  );
  const count = $derived(memberCount ?? members.length);
</script>

<section class="collection" aria-labelledby="collection-heading">
  <header class="collection-header">
    <div>
      <h2 id="collection-heading">{kindLabel || "Collection"}</h2>
      <p>{count} {count === 1 ? "member" : "members"}</p>
    </div>
    <svg width="20" height="20" viewBox="0 0 20 20" fill="none" aria-hidden="true">
      <path d="M2.5 4h5l1.5 2h8.5v10H2.5z" stroke="currentColor" stroke-width="1.5" stroke-linejoin="round" />
      <path d="M6 9h8M6 12h6" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    </svg>
  </header>

  {#if members.length === 0}
    <p class="collection-empty">
      Add page links to blocks below. Their block order becomes the collection order.
    </p>
  {:else}
    <ol class="member-list">
      {#each members as member (member.block_id)}
        <li>
          <button type="button" onclick={() => onNavigate(member.page_title)}>
            <span>{member.page_title}</span>
            <svg width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
              <path d="M5 3.5 9.5 8 5 12.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </button>
        </li>
      {/each}
    </ol>
  {/if}
</section>

<style>
  .collection {
    margin: 10px 0 22px;
    padding: 14px 16px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-secondary);
  }

  .collection-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    color: var(--text-secondary);
  }

  .collection-header h2 {
    margin: 0;
    color: var(--text-primary);
    font-size: 14px;
    font-weight: 650;
  }

  .collection-header p {
    margin: 2px 0 0;
    color: var(--text-secondary);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .collection-empty {
    margin: 13px 0 1px;
    color: var(--text-secondary);
    font-size: 12px;
    line-height: 1.5;
  }

  .member-list {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 12px 0 0;
    padding-left: 28px;
    color: var(--text-muted);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .member-list li {
    padding-left: 2px;
  }

  .member-list button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    min-height: 34px;
    gap: 12px;
    padding: 5px 8px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-primary);
    font: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
  }

  .member-list button:hover {
    background: var(--bg-hover);
    color: var(--text-link);
  }

  .member-list button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .member-list button span {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .member-list button svg {
    flex: 0 0 auto;
    color: var(--text-muted);
  }
</style>
