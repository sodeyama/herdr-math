# Calculus & linear algebra — pipe demo

Inline examples: $E=mc^2$, $\nabla \cdot \mathbf{E} = \rho/\varepsilon_0$, and $\det(A-\lambda I)=0$.

## Derivatives

$$\frac{d}{dx}\sin x = \cos x$$

$$\frac{d}{dx}\cos x = -\sin x$$

$$\frac{d}{dx} e^{ax} = a e^{ax}$$

$$\frac{d}{dx}\ln x = \frac{1}{x}$$

$$\frac{d}{dx}\left(x^n\right) = n x^{n-1}$$

$$\frac{d}{dx}\tan x = \sec^2 x$$

## Integrals

$$\int x^n\, dx = \frac{x^{n+1}}{n+1} + C \quad (n \neq -1)$$

$$\int e^{ax}\, dx = \frac{1}{a} e^{ax} + C$$

$$\int \frac{1}{x}\, dx = \ln|x| + C$$

$$\int \sin x\, dx = -\cos x + C$$

$$\int \cos x\, dx = \sin x + C$$

$$\int \sec^2 x\, dx = \tan x + C$$

## Taylor series

$$e^x = \sum_{n=0}^{\infty} \frac{x^n}{n!}$$

$$\sin x = \sum_{n=0}^{\infty} \frac{(-1)^n}{(2n+1)!} x^{2n+1}$$

$$\cos x = \sum_{n=0}^{\infty} \frac{(-1)^n}{(2n)!} x^{2n}$$

$$\ln(1+x) = \sum_{n=1}^{\infty} \frac{(-1)^{n+1}}{n} x^n$$

## Linear algebra

$$A\mathbf{x} = \mathbf{b}$$

$$\mathbf{x} = A^{-1}\mathbf{b} \quad \text{when } A \text{ is invertible}$$

$$\lambda\mathbf{v} = A\mathbf{v}$$

$$\|A\mathbf{x}\|_2 \le \|A\|_2 \|\mathbf{x}\|_2$$

$$\det(AB) = \det(A)\det(B)$$

$$A^\top A \text{ is symmetric positive semidefinite}$$

## Matrix factorizations

$$A = LU$$

$$A = QR$$

$$A = U\Sigma V^\top$$

$$A = Q\Lambda Q^\top \quad \text{(symmetric } A\text{)}$$

## Probability (bonus)

$$\mathbb{E}[X] = \sum_x x\, P(X=x)$$

$$\mathrm{Var}(X) = \mathbb{E}[X^2] - \mathbb{E}[X]^2$$

$$P(A\mid B) = \frac{P(B\mid A)\,P(A)}{P(B)}$$

$$f_X(x) = \frac{1}{\sigma\sqrt{2\pi}} \exp\!\left(-\frac{(x-\mu)^2}{2\sigma^2}\right)$$
