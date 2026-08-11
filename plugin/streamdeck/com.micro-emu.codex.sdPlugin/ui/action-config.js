(function () {
    const fallbackAction = document.body.dataset.defaultAction || "micro.act06";
    const actionSelect = document.querySelector('sdpi-select[setting="actionId"]');
    const iconSelect = document.querySelector('sdpi-select[setting="icon"]');
    const summaryName = document.querySelector(".summary-name");
    const summaryDescription = document.querySelector(".summary-description");
    const summaryRoute = document.querySelector(".summary-route");
    const automaticIcon = document.querySelector(".automatic-icon");
    const executorBadge = document.querySelector(".executor-badge");
    const mapStatus = document.querySelector(".map-status");
    const legacyWarning = document.querySelector(".legacy-warning");

    function actionOption(actionId) {
        return actionSelect?.querySelector(`option[value="${CSS.escape(actionId)}"]`)
            || actionSelect?.querySelector(`option[value="${CSS.escape(fallbackAction)}"]`);
    }

    function friendlyIcon(icon) {
        return String(icon || "action")
            .split("-")
            .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
            .join(" ");
    }

    function show(actionId, legacyIndex) {
        let resolvedId = actionId;
        const legacy = !resolvedId && Number.isInteger(Number(legacyIndex)) && Number(legacyIndex) <= 5;
        if (!resolvedId && Number.isInteger(Number(legacyIndex)) && Number(legacyIndex) >= 6 && Number(legacyIndex) <= 8) {
            resolvedId = `micro.act0${Number(legacyIndex)}`;
        }
        resolvedId = resolvedId || fallbackAction;
        const option = actionOption(resolvedId);
        const executor = option?.dataset.executor || "Micro";
        const map = option?.dataset.map || "";
        const label = option?.textContent?.trim() || "ACT06";
        const icon = option?.dataset.icon || "action";

        if (summaryName) summaryName.textContent = legacy ? `Legacy AG0${Number(legacyIndex)}` : label;
        if (summaryDescription) {
            summaryDescription.textContent = legacy
                ? "Compatibility mapping for an existing profile. Choose a catalog action to keep task selection and actions separate."
                : option?.dataset.description || "";
        }
        if (executorBadge) {
            executorBadge.textContent = legacy ? "Legacy" : executor;
            executorBadge.dataset.executor = legacy ? "Legacy" : executor;
        }
        if (automaticIcon) automaticIcon.textContent = legacy ? "Agent" : friendlyIcon(icon);
        if (summaryRoute) summaryRoute.hidden = false;
        if (legacyWarning) legacyWarning.hidden = !legacy;

        document.querySelectorAll(".mapped-key").forEach((key) => {
            const selected = Boolean(map) && key.dataset.map === map && !legacy;
            key.classList.toggle("selected", selected);
            key.setAttribute("aria-current", selected ? "true" : "false");
        });
        if (mapStatus) {
            mapStatus.textContent = legacy
                ? "Legacy AG profile preserved; Task Cards own the visible task positions."
                : map
                    ? `${label} maps to a physical Micro control.`
                    : `${label} is an extended action with no physical Micro position.`;
        }

        const automaticOption = iconSelect?.querySelector('option[value=""]');
        if (automaticOption) automaticOption.textContent = `Automatic — ${friendlyIcon(icon)}`;
    }

    show(fallbackAction);

    ["change", "input"].forEach((eventName) => {
        actionSelect?.addEventListener(eventName, () => show(actionSelect.value));
    });

    customElements.whenDefined("sdpi-select").then(async () => {
        await actionSelect?.updateComplete;
        const nativeSelect = actionSelect?.shadowRoot?.querySelector("select");
        nativeSelect?.addEventListener("change", () => show(nativeSelect.value));
        show(nativeSelect?.value || actionSelect?.value || fallbackAction);
    });

    const client = window.SDPIComponents?.streamDeckClient;
    if (!client) return;

    try {
        Promise.resolve(client.getSettings())
            .then((settings) => show(settings?.actionId, settings?.index))
            .catch(() => show(fallbackAction));

        client.didReceiveSettings?.subscribe((event) => {
            const settings = event?.payload?.settings ?? event?.settings ?? event;
            show(settings?.actionId, settings?.index);
        });
    } catch {
        show(fallbackAction);
    }
})();
