(function () {
    const select = document.querySelector('sdpi-select[setting="click"]');
    const hint = document.querySelector(".click-hint");

    function show(value) {
        if (!hint || !select) return;
        const option = select.querySelector(`option[value="${CSS.escape(value || "native")}"]`);
        hint.textContent = option?.dataset.description || "";
    }

    ["change", "input"].forEach((eventName) => {
        select?.addEventListener(eventName, () => show(select.value));
    });

    customElements.whenDefined("sdpi-select").then(async () => {
        await select?.updateComplete;
        const nativeSelect = select?.shadowRoot?.querySelector("select");
        nativeSelect?.addEventListener("change", () => show(nativeSelect.value));
        show(nativeSelect?.value || select?.value);
    });

    const client = window.SDPIComponents?.streamDeckClient;
    if (!client) return;

    Promise.resolve(client.getSettings())
        .then((settings) => show(settings?.click))
        .catch(() => show());
    client.didReceiveSettings?.subscribe((event) => {
        const settings = event?.payload?.settings ?? event?.settings ?? event;
        show(settings?.click);
    });
})();
