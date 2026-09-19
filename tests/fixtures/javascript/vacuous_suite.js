describe("VacuousTests", () => {
    it("empty test", () => {
    });

    it("tautology literal", () => {
        expect(1).toBe(1);
    });

    it("tautology boolean", () => {
        expect(true).toBeTruthy();
    });

    it("tautology string", () => {
        expect("abc").toEqual("abc");
    });
});
