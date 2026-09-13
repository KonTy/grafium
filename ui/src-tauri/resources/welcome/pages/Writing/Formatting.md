# Writing / Formatting

Use **bold** for an important distinction, *italics* for emphasis, and `inline code` for literal text. Headings and quotations help a long note stay readable.

> A useful explanation names its assumptions before showing its result.

## Code you can edit

```python
def observation_summary(clear_minutes, total_minutes):
    return clear_minutes / total_minutes

print(observation_summary(45, 60))
```

This is a code sample, not a command Grafium executes.

## Math in the note

For an ideal circular orbit, $v = \sqrt{GM/r}$. Increasing the radius reduces the required circular speed when the central mass stays fixed.

$$
T = 2\pi\sqrt{\frac{r^3}{GM}}
$$

KaTeX renders the notation. Explain the symbols in words too: $T$ is the period, $r$ is the orbital radius, $M$ is the central mass, and $G$ is the gravitational constant.

## A diagram beside the explanation

![Original sketch of gravity and sideways motion](../assets/welcome/orbit.svg)

This is a real local SVG, bundled with the sample. A diagram can clarify the geometry while the text keeps its assumptions visible.

Continue with [[Writing/Tables]] or connect the math to [[Space/Orbits/Gravity]].
