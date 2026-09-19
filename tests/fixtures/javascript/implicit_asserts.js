describe("ImplicitAssertions", () => {
    it("throws error", () => {
        expect(() => { throw new Error("fail"); }).toThrow("fail");
    });

    it("handles promise resolution", async () => {
        await expect(Promise.resolve(42)).resolves.toBe(42);
    });

    it("handles promise rejection", async () => {
        await expect(Promise.reject(new Error("boom"))).rejects.toThrow();
    });
});
