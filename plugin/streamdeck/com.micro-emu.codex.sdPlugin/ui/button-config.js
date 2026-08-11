(function () {
    const fallbackIndex = Number(document.body.dataset.defaultIndex || 6);
    const select = document.querySelector('sdpi-select[setting="index"]');
    const status = document.querySelector(".map-status");

    function codeFor(index) {
        return index <= 5
            ? `AG${String(index).padStart(2, "0")}`
            : `ACT${String(index).padStart(2, "0")}`;
    }

    function show(index) {
        const parsed = Number(index);
        const current = Number.isInteger(parsed) && parsed >= 0 && parsed <= 8
            ? parsed
            : fallbackIndex;
        document.querySelectorAll(".mapped-key").forEach((key) => {
            const selected = Number(key.dataset.index) === current;
            key.classList.toggle("selected", selected);
            key.setAttribute("aria-current", selected ? "true" : "false");
        });
        if (status) status.textContent = `${codeFor(current)} selected`;
    }

    show(fallbackIndex);

    ["change", "input"].forEach((eventName) => {
        select?.addEventListener(eventName, () => show(select.value));
    });

    customElements.whenDefined("sdpi-select").then(async () => {
        await select?.updateComplete;
        const nativeSelect = select?.shadowRoot?.querySelector("select");
        nativeSelect?.addEventListener("change", () => show(nativeSelect.value));
        show(nativeSelect?.value ?? select?.value ?? fallbackIndex);
    });

    const client = window.SDPIComponents?.streamDeckClient;
    if (!client) return;

    try {
        Promise.resolve(client.getSettings())
            .then((settings) => show(settings?.index ?? fallbackIndex))
            .catch(() => show(fallbackIndex));

        client.didReceiveSettings?.subscribe((event) => {
            const settings = event?.payload?.settings ?? event?.settings ?? event;
            show(settings?.index ?? fallbackIndex);
        });
    } catch {
        show(fallbackIndex);
    }
})();
