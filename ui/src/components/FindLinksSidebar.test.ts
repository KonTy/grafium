import { describe, expect, it } from "vitest";
import appSource from "../App.svelte?raw";
import pageContentSource from "./PageContent.svelte?raw";
import referencePanelSource from "./ReferencePanel.svelte?raw";
import sidebarSource from "./Sidebar.svelte?raw";

describe("right-panel find links action", () => {
  it("routes the References tab action through the existing PageContent suggestion workflow", () => {
    expect(sidebarSource).not.toContain("<span>Find links</span>");
    expect(sidebarSource).not.toContain("onFindLinks");

    expect(referencePanelSource).toContain("onFindLinks?: (page: { id: string; title: string }) => void");
    expect(referencePanelSource).toContain("function findLinksForCurrentPage()");
    expect(referencePanelSource).toContain("Find links");

    expect(appSource).toContain('function handleFindLinksForPage(page: Pick<Page, "id">)');
    expect(appSource).toContain('new CustomEvent("page-content-find-links"');
    expect(appSource).toContain("onFindLinks={handleFindLinksForPage}");

    expect(pageContentSource).toContain('window.addEventListener("page-content-find-links"');
    expect(pageContentSource).toContain("void handleSuggestLinks();");
    expect(pageContentSource).toContain("const linkCandidateGroups = $derived.by(() => groupLinkCandidates(linkCandidates));");
    expect(pageContentSource).toContain("{#each linkCandidateGroups as group (group.key)}");
    expect(pageContentSource).toContain("handleAcceptLinkCandidateGroup(group)");
    expect(pageContentSource).toContain("Link all");
    expect(pageContentSource).toContain("Fix + link");
    expect(pageContentSource).toContain("Alias match");
    expect(pageContentSource).toContain("alias / canonical target");
    expect(pageContentSource).toContain("linkCandidateContextPreview(group)");
    expect(pageContentSource).toContain("handleRevealLinkCandidateGroup(group)");
    expect(pageContentSource).toContain("Jump to this occurrence");
    expect(pageContentSource).not.toContain("linkCandidates.slice(0, 8)");
  });
});
